import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useLocation, useNavigate, useParams } from 'react-router'
import { Copy, Forward, Loader2, Trash2 } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { ScrollArea } from '@/components/ui/scroll-area'

/**
 * **同一个 id 出现两次**时说出来（每个 id 只说一次，免得刷屏）。
 *
 * 为什么不是 `console.warn` 就算了：两条同 id 的行会被画在**同一个位置**上，
 * 人看到的是"字叠在一起、看不清楚"—— 而这件事在控制台里必须留一句话、带上 id，
 * 下一次才追得到它是从哪条路进来的（2026-10-16 真机上就是这么撞见的）。
 */
const reportedDuplicateRowIds = new Set<string>()
function reportDuplicateRowId(id: string): void {
  if (reportedDuplicateRowIds.has(id)) return
  reportedDuplicateRowIds.add(id)
  console.error(
    `[chat] 同一条消息在时间线里出现了两次（id=${id}）—— 重复的行身份会让两行叠在一起；` +
      '已只渲染一条，请把这一行报出来。',
  )
}

/**
 * 主体进行态刷新间隔（毫秒）。**为什么是拉取**：进行态没有推送通道——
 * 独立 Live WS 随旧 Live State 删除（2026-09-28）；复用人机各自已有的连接
 * 推一条无事实的 `live.available` 是另一刀的事。5 秒是"能看出人在不在写"
 * 与"别把世界问烦"之间的取舍；"正在写"最多晚一个周期亮。
 */
const LIVE_REFRESH_MS = 5000

/** 「以下是新消息」那条分隔线的元素 id（进房落点用它 —— 走 `lib/anchorScroll.ts` 那份共用实现）。 */
const UNREAD_ANCHOR_ID = 'unread-anchor'
/** 每一条消息那一行的元素 id 前缀（跳到某一条、深链接都靠它）。 */
const MESSAGE_ANCHOR_PREFIX = 'm-'

/**
 * 「列表滚不动也要读得到」那一档的**上限**（页）。
 *
 * 它不是功能边界，是"别把世界问烦"的闸：正常窗口下，一屏装得下 5 页 × 50 条消息很少见
 * （真撞上，人往上滚一次就继续读 —— 自动那一路照旧）。
 */
const AUTO_FILL_MAX_PAGES = 5

import { ConversationList } from '@/components/celira/ConversationList'
import { ChatHeader } from '@/components/celira/ChatHeader'
import { ChatWallpaper } from '@/components/celira/ChatWallpaper'
import { MemoizedMessageBubble } from '@/components/celira/MessageBubble'
import { loadHiddenMessages, saveHiddenMessages } from '@/api/hiddenMessages'
import { addHidden, isHidden, removeHidden } from '@/lib/hiddenMessages'
import { scrollToAnchor } from '@/lib/anchorScroll'
import { copyTextOf, isSelected, selectedInOrder, toggleSelected } from '@/lib/messageSelection'
import { MessageInput } from '@/components/celira/MessageInput'
import { DateDivider } from '@/components/celira/DateDivider'
import { UnreadDivider } from '@/components/celira/UnreadDivider'
import { ConnectionBanner } from '@/components/celira/ConnectionBanner'
import { NewMessagePill } from '@/components/celira/NewMessagePill'
import { RoomInfoDrawer } from '@/components/celira/RoomInfoDrawer'
import { EmptyRoomsState } from '@/components/celira/EmptyRoomsState'
import { chatFeedState } from '@/api/uiPolicy'
import { NewRoomSheet } from '@/components/celira/NewRoomSheet'
import { ForwardSheet } from '@/components/celira/ForwardSheet'
import { SystemMessage } from '@/components/celira/SystemMessage'
import { ImagePreviewLightbox, type PreviewImage } from '@/components/celira/ImagePreviewLightbox'
import type { Message, Participant } from '@/api/domain'
import {
  useAppStore,
  useConnection,
  useRoomMessages,
  useRooms,
} from '@/api/store'
import { tryGetParticipant } from '@/api/participantDirectory'
import { toParticipant } from '@/api/participantProjection'
import type { ReplyTargetState } from '@/components/celira/ReplyQuote'
import { isNearBottomOf, landingPlanFor, resolveSelfId, shouldFollowToBottom } from '@/api/selfIdentity'
import { saveLastRoom } from '@/lib/wallpaper'
import { cn } from '@/lib/utils'
import { roomActionErrorText } from '@/api/contractErrors'

export function ChatPage() {
  const { roomId } = useParams()
  const navigate = useNavigate()
  const [infoOpen, setInfoOpen] = useState(false)
  const [newRoomOpen, setNewRoomOpen] = useState(false)
  const [replyTo, setReplyTo] = useState<Message | null>(null)
  const [previewOpen, setPreviewOpen] = useState(false)
  const [previewIndex, setPreviewIndex] = useState(0)
  /**
   * ★★ **时间线的滚动容器要"现找"，不能存 ref**（2026-10-16 真机 + 探针一起定的案）。
   *
   * 事故形状：`ScrollArea` 的视口节点会被 **Radix 在 React 背后换掉** ——
   * 对象 ref 与回调 ref 都会继续指着**那个已经被摘下来的旧节点** ✗。
   * 症状（实测）：`vp.scrollTop` 永远读到旧值（≥80）⇒ "滚到顶就自动读更早"**永远不触发**；
   * 同一个原因也会让"跟着滚到底 / 翻页钉位 / 跳到某一条"全部对着一个死节点使劲。
   *
   * 判据：**这一层包装 div 是页面自己的、稳定的**（React 不会换它），视口是它的后代
   * ⇒ 每次用的时候从它里面查一次。属性名用的是仓库里**已有的那个约定**
   * （`data-radix-scroll-area-viewport`，本文件上面那些 Tailwind 选择器就在用它）。
   */
  const panelRef = useRef<HTMLDivElement | null>(null)
  const timelineViewport = useCallback(
    () =>
      panelRef.current?.querySelector<HTMLElement>('[data-radix-scroll-area-viewport]') ?? null,
    [],
  )
  // 未读锚点（「以下是新消息」分隔线基准消息 id）：换房重置、进房后捕获一次
  const unreadAnchorRoomRef = useRef<string | null>(null)
  const [unreadAnchorId, setUnreadAnchorId] = useState<string | null>(null)
  const lastMessageIdRef = useRef<string | null>(null)
  // 路由当前房 id（ref 形式）：给需要「仍是当前房间」异步终止条件的回调
  // （onLoadReplyTarget 补早）做惰性所有权校验，不随闭包捕获旧值。
  const activeRoomIdRef = useRef(roomId)
  // Keep route ownership current during render as well as in the lifecycle
  // cleanup effect below; a route switch must not make the first post-commit
  // read attempt observe the previous room.
  activeRoomIdRef.current = roomId
  const connection = useAppStore((s) => s.connection)
  const refreshRoomMembers = useAppStore((s) => s.refreshRoomMembers)
  const refreshRoomLive = useAppStore((s) => s.refreshRoomLive)
  const refreshRoom = useAppStore((s) => s.refreshRoom)
  const membersByRoom = useAppStore((s) => s.membersByRoom)
  // 「我拥有的 Agent」（空状态指路要用；2026-10-05）。这一格由 store 在连接时刷新（`refreshOwnedAgents`），
  // 页面只读它 —— 不在页面里另发一次请求（同一类事实一处来路）。
  const ownedAgents = useAppStore((s) => s.ownedAgents)
  const [flashId, setFlashId] = useState<string | null>(null)
  const [editRequest, setEditRequest] = useState<{ message: Message; seq: number } | null>(null)
  const [forwardTargets, setForwardTargets] = useState<Message[]>([])
  const allRooms = useRooms()
  const activeRoom = allRooms.find((r) => r.id === roomId)
  // 记录最后停留的会话，供设置页壁纸「仅此会话」作用范围使用。
  // 只用原始 id/name 值做依赖，不依赖整个 activeRoom 对象。
  const lastRoomId = activeRoom?.id
  const lastRoomName = activeRoom?.name
  useEffect(() => {
    // 落盘是异步的（存储窄口）：这里不 await —— 它只是"设置页作用范围"的提示值。
    if (lastRoomId && lastRoomName) void saveLastRoom({ id: lastRoomId, name: lastRoomName })
  }, [lastRoomId, lastRoomName])
  const { me } = useConnection()
  const roomMessages = useRoomMessages(roomId ?? '')
  // 首屏读数（**缺省不是事实**）：`messagesByRoom` 的**键在不在**才是"到没到"，
  // 空数组只是"到了、而且确认没有消息"。旧实现只有 `roomMessages.length === 0`，
  // 于是"还没到 / 读失败 / 连接不在"三种处境都被说成「这个会话还没有消息」。
  const messagesArrived = useAppStore((s) => (roomId ? s.messagesByRoom[roomId] !== undefined : false))
  const messagesLoadError = useAppStore((s) => (roomId ? (s.messagesLoadErrorByRoom[roomId] ?? null) : null))
  // 首屏那一块的四态（**只有 `empty` 那一支允许说"还没有消息"**）：判据与文案都在
  // `chatFeedState` 一处（纯函数，`tests/chat-feed.test.ts` 钉着）。
  // `canRead` 与 `loadRoomMessages` 的触发条件同一判据 —— 连接不在时**根本没发起读取**，
  // 那不是"正在读取"，更不是"没有消息"。
  const chatFeed = chatFeedState({
    arrived: messagesArrived,
    count: roomMessages.length,
    error: messagesLoadError,
    canRead: connection === 'connected' || connection === 'reconnecting',
  })
  const send = useAppStore((s) => s.send)
  const loadRoomMessages = useAppStore((s) => s.loadRoomMessages)
  const recallMessage = useAppStore((s) => s.recall)
  const retryMessage = useAppStore((s) => s.retry)

  /**
   * **本机隐藏的消息**（「删除 · 仅为我」，2026-10-08）。
   *
   * 它是**视图**不是事实：世界里那条消息一动没动（撤回才是公共动作）。所以名单存在本机
   * （`api/hiddenMessages.ts`，世界作用域 ＋ 房间级），渲染时滤掉；读不到按空名单处理。
   */
  const [hidden, setHidden] = useState<string[]>([])
  /** 被隐藏的那些 id（给"撤销"用）：撤销一次把这一批都还回来。 */
  const [hideUndo, setHideUndo] = useState<string[] | null>(null)
  const [hideError, setHideError] = useState<string | null>(null)
  /** **多选态**（2026-10-08）：`selectMode` 是"在不在多选"，`selected` 是选了哪几条。 */
  const [selectMode, setSelectMode] = useState(false)
  const [selected, setSelected] = useState<string[]>([])
  const [selectNote, setSelectNote] = useState<string | null>(null)
  useEffect(() => {
    let alive = true
    setHideUndo(null)
    setHideError(null)
    setSelectMode(false)
    setSelected([])
    if (!roomId) {
      setHidden([])
      return
    }
    void loadHiddenMessages(roomId).then((list) => {
      if (alive) setHidden(list)
    })
    return () => {
      alive = false
    }
  }, [roomId])
  /** 只滤"看得见的那一份"；`chatFeed` 仍按**世界里的条数**判（见下面那条"都被你隐藏了"）。 */
  const visibleMessages = useMemo(
    () => roomMessages.filter((each) => !isHidden(hidden, each.id)),
    [roomMessages, hidden],
  )
  const writeHidden = async (next: string[]) => {
    setHidden(next)
    if (!roomId) return false
    const saved = await saveHiddenMessages(roomId, next)
    if (!saved) setHideError('没能保存（刷新之后它还会回来）')
    else setHideError(null)
    return saved
  }
  const handleHide = (message: Message) => {
    setHideUndo([message.id])
    void writeHidden(addHidden(hidden, message.id))
  }
  const undoHide = async () => {
    const ids = hideUndo
    if (!ids || ids.length === 0) return
    setHideUndo(null)
    let next = hidden
    for (const id of ids) next = removeHidden(next, id)
    await writeHidden(next)
  }

  // ── 多选（2026-10-08）：判据在 `lib/messageSelection.ts`，这里只接 ────────────────

  const exitSelectMode = () => {
    setSelectMode(false)
    setSelected([])
  }
  const enterSelectMode = (message: Message) => {
    setSelectMode(true)
    setSelected([message.id])
  }
  /** **按时间线**取那批消息（不是按点击先后）—— 顺序就是"对面读到的顺序"。 */
  const selectedMessages = selectedInOrder(roomMessages, selected)
  const startBatchForward = () => {
    if (selectedMessages.length === 0) return
    setForwardTargets(selectedMessages)
    exitSelectMode()
  }
  const copySelected = async () => {
    const text = copyTextOf(selectedMessages)
    exitSelectMode()
    if (text === '') {
      setSelectNote('这几条没有正文可复制')
      setTimeout(() => setSelectNote(null), 4000)
      return
    }
    try {
      await navigator.clipboard.writeText(text)
      setSelectNote(`已复制 ${selectedMessages.length} 条`)
    } catch {
      setSelectNote('没能复制（浏览器没给剪贴板权限）')
    }
    setTimeout(() => setSelectNote(null), 4000)
  }
  const hideSelected = async () => {
    const ids = selectedMessages.map((each) => each.id)
    exitSelectMode()
    if (ids.length === 0) return
    setHideUndo(ids)
    let next = hidden
    for (const id of ids) next = addHidden(next, id)
    await writeHidden(next)
  }
  const retryConnection = useAppStore((s) => s.retryConnection)
  const markRoomRead = useAppStore((s) => s.markRoomRead)
  const loadOlder = useAppStore((s) => s.loadOlder)

  // ── 回复 / 撤回（Slice 4）：真实身份、当前已加载目标、稳定错误 ──
  // 参与者显示名解析：优先真实连接身份（me / 房间参与者投影）；真实连接
  // 绝不查 fixtures；Mock/Core 无连接身份时才回退 fixtures。
  const participantNameOf = useCallback(
    (authorId: string): string | undefined => {
      if (me?.participant?.id === authorId) return me.participant.display_name
      const room = allRooms.find((r) => r.id === roomId)
      const p = room?.participants?.find((x) => x.id === authorId)
      if (p) return p.displayName
      const fp = tryGetParticipant(authorId)
      if (fp) return fp.displayName
      return undefined
    },
    [me, allRooms, roomId],
  )

  // 作者投影解析器（给 MessageBubble / MessageContextMenu）：
  // 真实 me + 当前房间参与者投影优先；真实连接下未知作者返回 undefined → 组件
  // 显示中性「参与者」，绝不猜 human/agent（不显示 Agent 徽章/主页门禁、
  // 不判为自己）；Mock/Core 无连接身份时回退 fixtures。
  const resolveAuthor = useCallback(
    (authorId: string): Participant | undefined => {
      if (me?.participant?.id === authorId) {
        return toParticipant(me.participant)
      }
      const room = allRooms.find((r) => r.id === roomId)
      const p = room?.participants?.find((x) => x.id === authorId)
      if (p) return p
      return tryGetParticipant(authorId) ?? undefined
    },
    [me, allRooms, roomId],
  )

  // 房间成员快照（@ 高亮与候选共用）：**唯一一份投影**，已提交消息（MessageBubble）
  // 与输入区共用同一条名表通路，不单开第二条。真实世界下这里是
  // 真实成员投影（group 走 members 端点），投影走 `toParticipant` 唯一权威；缺省时由
  // MentionText 的闸门决定（真实世界不回退演示名）。**带 id**：结构化提及按 id 取种类。
  const mentionMembers = useMemo(
    () =>
      roomId
        ? (membersByRoom[roomId] ?? []).map((m) => {
            const p = toParticipant(m.participant)
            return { id: p.id, name: p.displayName, kind: p.kind }
          })
        : undefined,
    [membersByRoom, roomId],
  )

  // ReplyQuote 目标解析器：只从当前已加载的权威 timeline 解析目标，绝不查
  // fixtures；目标已撤回（tombstone）→ recalled 占位，绝不泄露撤销前正文；
  // 未加载 → unloaded 占位（点击由 onLoadReplyTarget 按 opaque 分页补早）。
  const resolveReplyTarget = useCallback(
    (messageId: string): ReplyTargetState => {
      const target = roomMessages.find((m) => m.id === messageId)
      if (!target) return { status: 'unloaded' }
      if (target.recalledAt != null) {
        return { status: 'recalled', authorName: participantNameOf(target.authorId) }
      }
      return {
        status: 'loaded',
        message: target,
        authorName: participantNameOf(target.authorId) ?? '参与者',
      }
    },
    [roomMessages, participantNameOf],
  )

  // 目标未加载时的补早：按既有 opaque `before` 分页逐页加载更早消息直到找
  // 到或没有更旧页（绝不猜 cursor、绝不无限循环）。有界终止条件 = **cursor 仍
  // 前进**（每轮 loadOlder 返回刚加载条数 >0、nextBefore 仍非空）**且当时的
  // 当前房仍是目标房**（activeRoomIdRef 惰性校验）——换房/换代（loadOlder 内
  // 的 session 所有权校验使旧代返回 0）即中止；不新增单消息接口。
  const onLoadReplyTarget = useCallback(
    async (messageId: string) => {
      const targetRoom = roomId ?? ''
      if (!targetRoom) return
      for (;;) {
        if (activeRoomIdRef.current !== targetRoom) return
        const loaded = useAppStore.getState().messagesByRoom[targetRoom] ?? []
        if (loaded.some((m) => m.id === messageId)) {
          jumpToMessage(messageId)
          return
        }
        const before = useAppStore.getState().nextBeforeByRoom[targetRoom]
        if (!before) return
        const prepended = await loadOlder(targetRoom)
        if (prepended <= 0) return
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [roomId, loadOlder],
  )

  // 从「查找聊天记录」跳入：消费一次性 targetId，复用回复定位的补早 + 跳转
  // 主链；replace 清空 state 防刷新/回退重复触发。
  const location = useLocation()
  useEffect(() => {
    const targetId = (location.state as { targetId?: string } | null)?.targetId
    if (!targetId || !roomId) return
    navigate(`/app/chat/${roomId}`, { replace: true, state: null })
    void onLoadReplyTarget(targetId)
  }, [location.state, roomId, navigate, onLoadReplyTarget])

  // 撤回失败：保留当前权威 timeline，**不假成功**，但**也不再静默**（TASK_72 追补）。
  //
  // 背景：TASK_63 刀 C 删掉了唯一的失败呈现（那条错误条原先挂在 Host 输入区里）⇒
  // core 直连下撤回失败**从来没有可见反馈**（`catch {}` 把它吞掉）。而"稳定分类 → 人话"
  // 的映射一直在 `api/contractErrors.ts` 里 ⇒ 那两条**成了死映射**（这条路上没有消费者）。
  // 这一刀只做一件事：**把已有映射接到这条路上**（超窗 / 无权各给一句人话）。
  // 不制造本地消息事实（权威 timeline 仍由 store 重读），也不改任何契约。
  const [actionError, setActionError] = useState<string | null>(null)
  const handleRecall = useCallback(
    async (message: Message) => {
      if (!roomId) return
      setActionError(null)
      try {
        await recallMessage(roomId, message.id)
      } catch (error) {
        // 红路径**必须说人话**：稳定分类 → `contractErrors` 里那两句（未知分类走通用文案）。
        setActionError(roomActionErrorText(error))
      }
    },
    [roomId, recallMessage],
  )

  // 切换房间/会话 epoch 换代/目标撤回时清理或校正本地 replying 状态：目标
  // 已不再加载（换房/换代重读窗口）或目标已变为 tombstone → 清空引用条，
  // 不得回复到已撤回/不可见目标。
  useEffect(() => {
    if (!replyTo) return
    const target = roomMessages.find((m) => m.id === replyTo.id)
    if (!target || target.recalledAt != null) setReplyTo(null)
  }, [replyTo, roomMessages])

  /**
   * **读更早的消息**（2026-10-16 改成**主流形状**：主人当次："加载更早消息这个，你觉得有必要吗…
   * 我看其他 im 好像没有这个选项吧。一般是自适应的那种？"）。
   *
   * 三件事，一件都不许少：
   * 1. **滚到顶就自动读**（`onScroll` 里那一条：`scrollTop < 80` 且有更早的分页）；
   * 2. **读的时候顶部给指示**（`olderLoading` ⇒ 那一行「正在读更早的消息…」）——
   *    主流就是这样：**没有按钮，只有一个转圈**；
   * 3. **滚不动也要读得到**（下面那条兜底）：一屏装得下（列表不可滚）但还有更早的分页时，
   *    光靠"滚到顶"永远触发不了 ⇒ 那时**自动补一页**，直到要么能滚、要么没有更早的了。
   *    ⚠ 上限 5 页：这是"别把世界问烦"的闸，不是功能边界（正常一屏装得下 5 页 × 50 条的窗口很少见）。
   *
   * ⚠ 从前这里还有一颗**浮动按钮**「加载更早消息」—— 撤掉了：它与 1、2 重复，
   * 而且是主流 IM 里不存在的形状（判据在 `tests/no-silent-red-path.test.ts`）。
   */
  const [olderLoading, setOlderLoading] = useState(false)
  const [olderError, setOlderError] = useState<string | null>(null)
  const olderLoadingRef = useRef(false)
  /** 「列表滚不动也要读得到」那一档已经自动补了几页（换房清零）。 */
  const autoFillRef = useRef(0)

  const loadEarlier = async () => {
    if (olderLoadingRef.current) return 0
    olderLoadingRef.current = true
    setOlderLoading(true)
    setOlderError(null)
    const vp = timelineViewport()
    const before = vp ? { top: vp.scrollTop, height: vp.scrollHeight } : null
    try {
      const prepended = await loadOlder(roomId ?? '')
      if (prepended > 0 && before) pinTopAfterPrepend(before)
      return prepended
    } catch (error) {
      // ★ **读更早失败不许静默**（2026-10-16）：从前 store 里那句 `catch { return 0 }` 把
      // `this` 丢失的 TypeError 吞了整整一路 ⇒ "加载更早"**从来没成功过**，界面上毫无迹象。
      // 现在说人话 + 给重试；游标不动 ⇒ 重试就是同一页，不跳过任何历史。
      setOlderError(roomActionErrorText(error))
      return 0
    } finally {
      olderLoadingRef.current = false
      setOlderLoading(false)
    }
  }

  /**
   * **翻页之后把原来顶部那一行钉回原处**（2026-10-16 主人真机反馈："加载更早消息，这个是不是
   * 有问题呢…要么是显示位置有问题，不太明显那种，要么就是机制有问题？"）。
   *
   * 从前是按**行索引**落（`scrollToIndex(prepended, {align:'start'})`）—— 那要求"量出来的行高"
   * 准确，而真机实测它**从未生效**（见下面 `rows` 头上那段）⇒ 落点会偏一点。
   * 现在按**高度差**钉：进这一趟之前记下 `scrollTop / scrollHeight`，前插之后把
   * `scrollTop = 旧top + (新高度 - 旧高度)` 逐帧重设 —— 它是**绝对目标**（不是增量），
   * 每帧都在纠偏、不累积漂移；文档流里这个式子就是精确的。
   */
  const pinTopAfterPrepend = (before: { top: number; height: number }) => {
    let frames = 10
    const pin = () => {
      const el = timelineViewport()
      if (!el) return
      el.scrollTop = before.top + (el.scrollHeight - before.height)
      if (--frames > 0) requestAnimationFrame(pin)
    }
    requestAnimationFrame(pin)
  }

  /**
   * ★ **落点怎么落**（2026-10-16）。判据在 `selfIdentity.landingPlanFor`（纯逻辑、有用例）：
   * 有未读分隔线 ⇒ 落分隔线；没有 ⇒ 落**最后一条**（= 滚到底）。
   *
   * ⚠ 为什么不再用虚拟列表的 `scrollToIndex`：那一套要求"量出来的行高"准确，而真机实测它
   * **从未生效**（整屏按估值摆位 ⇒ 行行相压，专测探针量到 180 处重叠）⇒ 消息列表已经回到
   * **普通文档流** ⇒ 落点直接对 DOM 说：**分隔线交给 `lib/anchorScroll.ts` 那份共用实现**
   * （带锚点的滚动全仓只许一份），**最新一条**用 `scrollTop = scrollHeight`
   * —— 两者都不依赖任何测量值。
   */
  const applyLanding = (plan: { kind: 'divider' | 'latest' }) => {
    const vp = timelineViewport()
    if (!vp) return
    if (plan.kind === 'divider') {
      // 分隔线那一档走**共用实现**（`lib/anchorScroll.ts`：有界等待 + 出现即收工）——
      // 带锚点的滚动全仓只许一份，判据在 `tests/first-run-landing.test.ts`。
      scrollToAnchor(UNREAD_ANCHOR_ID)
      return
    }
    vp.scrollTop = vp.scrollHeight
  }

  const landingRef = useRef(false)

  /**
   * **落定**：头几帧里重复落，直到它真的稳住。
   *
   * 为什么必须重复：进房那一刻**行高还会变**（中文长段重排、字体就绪、图片/附件晚到）
   * ⇒ 滚一次会被后面的增长甩在后面。判据：**到过底又离开 ⇒ 是用户自己往上翻了 ⇒ 立刻停手**
   * （不与用户抢滚动）；一直没到底 ⇒ 继续试满这一小段窗口（约 400ms，只是几次赋值）。
   */
  const settleLanding = (plan: { kind: 'divider' | 'latest' }, frames = 24) => {
    landingRef.current = true
    let sawBottom = false
    const step = (left: number) => {
      if (!landingRef.current) return
      applyLanding(plan)
      if (isNearBottom()) sawBottom = true
      else if (sawBottom) {
        landingRef.current = false
        return
      }
      if (left > 1) requestAnimationFrame(() => step(left - 1))
      else landingRef.current = false
    }
    step(frames)
  }

  /** 落到**最新一条**（新消息跟随、那颗「N 条新消息」都走它）。 */
  const scrollToBottom = () => {
    settleLanding({ kind: 'latest' }, 6)
  }

  /**
   * 「我离底部近吗」—— **一屏之内都算近**（2026-10-05 主人真机反馈改的；判据本体与用例在
   * `selfIdentity.isNearBottomOf`）。原先只有 64px：对方回一段长话就把人推出去了 ⇒ 新消息来了
   * **既不跟随滚动、也不推进读游标**（下面那个 effect 把两件事写在同一个 `if` 里）。
   */
  const isNearBottom = () => {
    const vp = timelineViewport()
    if (!vp) return true
    return isNearBottomOf({
      scrollHeight: vp.scrollHeight,
      scrollTop: vp.scrollTop,
      clientHeight: vp.clientHeight,
    })
  }

  /**
   * ★★ **消息列表回到普通文档流**（2026-10-16 真机事故之后）。
   *
   * ## 为什么把虚拟列表拆掉
   *
   * 从前的形状：每一行 `position: absolute` + `translateY(量出来的高度累加)`。
   * 2026-10-16 真机连续两次拍到"同一段字被画了两遍、只差几像素"，第二次整屏垮掉。
   * 为此立了专测探针（`scripts/home/probe-chat-layout.js`：量每一行的矩形，机械地检出重叠），
   * 一次就量到 **180 处重叠**、行距**恒为 84px**（= `estimateSize` 的估值，而真实行高 90–230px）
   * ⇒ **实测行高从未生效**，所有行都按估值摆位。
   *
   * 这条路在这个文件里已经留了一串伤口（数字下标串行、切页截断、空洞与重叠……），
   * 换来的只是"长房间少渲染几屏"。判据：**一条时间线最多几百条（一页 50、翻页也是 50 一页），
   * 普通文档流完全撑得住；而"行位靠测量值"一旦不成立，坏的是整个画面。**
   * ⇒ 行不再绝对定位，**没有任何"按量出来的高度摆位"**，这一整类症状从构造上不可能再发生。
   *
   * ⚠ 代价如实记：极长的会话（翻十几页之后上千条）DOM 会变重；真到那一步再谈窗口化，
   * 而且要换一种**不依赖行高测量**的做法（例如按"页"窗口，页内正常流）。
   *
   * ⚠ **行身份 = 消息 id ⇒ id 必须唯一**：真出现重复 id 时**只渲染一条**并**说出来**
   * （`reportDuplicateRowId`）—— 重复身份会让两行叠在一起，而那正是"看着不清楚"的成因之一。
   */
  const rows = useMemo(() => {
    const seen = new Set<string>()
    const built: {
      key: string
      message: (typeof visibleMessages)[number]
      compact: boolean
      dayChanged: boolean
      unreadStart: boolean
    }[] = []
    let prev: (typeof visibleMessages)[number] | undefined
    for (const m of visibleMessages) {
      if (seen.has(m.id)) {
        reportDuplicateRowId(m.id)
        continue
      }
      seen.add(m.id)
      built.push({
        key: m.id,
        message: m,
        compact: !m.system && !!prev && !prev.system && prev.authorId === m.authorId,
        dayChanged: !!m.day && prev?.day !== m.day,
        unreadStart: !m.system && m.id === unreadAnchorId,
      })
      prev = m
    }
    return built
  }, [visibleMessages, unreadAnchorId])

  /**
   * **我在这间房说的最后一条**（——「已读 / 未读」只挂在这一条上）。
   *
   * 为什么是"最后一条"而不是"每一条"：服务端那格是**房间级的一个布尔**
   * （`peer_read`，契约 `CORE_V1_READ_RECEIPT.md` §2），判据 = 对方的读位置
   * 是否过了**我最后那条**。给每条气泡都带 ✓✓ 就是"逐条回执"——契约明文不做，
   * 那是另一格事实，不是这里能派生的（我们只有位置，没有逐条确认）。
   *
   * ⚠ 即使后面还有对方的消息，这一格**仍然留在我最后那条上**（像 WhatsApp 的
   * ✓✓：对方回了也不会消失）。
   */
  const myLastMessageId = useMemo(() => {
    const selfId = me?.participant?.id
    if (!selfId) return null
    for (let i = rows.length - 1; i >= 0; i -= 1) {
      const m = rows[i].message
      if (!m.system && m.authorId === selfId) return m.id
    }
    return null
  }, [rows, me?.participant?.id])

  /**
   * ★ **滚到顶 ⇒ 自动读更早的**（主流形状：没有按钮，顶部一个转圈）。
   *
   * ⚠⚠ **2026-10-16 实测：这条曾经从未生效**（专测探针量到"滚到顶 0 次翻页请求、
   * 顶部指示也没出现"，而服务端明明说还有更早的）。
   *
   * 两次踩坑都记在这里，因为它们长得不一样、坏起来一模一样：
   * ① 原来它写成 `useEffect(() => { const vp = timelineViewport(); if (!vp) return; … }, [roomId])`
   *    —— 第一次跑时 `activeRoom` 还没到 ⇒ `ScrollArea` 没挂载 ⇒ `vp` 是 null ⇒ 早退之后再没跑过 ✗；
   * ② 改成"有界重试到视口出现"之后**仍然不生效**：`ScrollArea` 的视口节点会被重建
   *    （Radix 内部行为），监听留在**被换掉的那个节点**上 ⇒ 现役节点上没有监听 ✗。
   *
   * ⇒ 结论：**监听不能挂在会被换掉的节点上**。现在挂在**这一层稳定的包装 div** 上
   * （React 的 `onScroll` 按捕获阶段收子孙的滚动事件，`scroll` 本身不冒泡也没关系），
   * 事件里现读 `timelineViewport()` —— 谁在滚、滚到哪儿，都以**当下**为准。
   */
  const onTimelineScroll = useCallback(() => {
    const vp = timelineViewport()
    if (!vp || vp.scrollTop >= 80) return
    // 没有更早的分页时不触发，避免空跑的锚定回写与用户滚轮打架
    if (!useAppStore.getState().nextBeforeByRoom[roomId ?? '']) return
    void loadEarlier()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [roomId])

  /**
   * ★★ **滚动监听挂在 `document` 的捕获阶段**（2026-10-16 第四次修正 —— 前三次都记在上面：
   * ① effect 里挂 ⇒ 视口还没挂载就早退；② 有界重试挂 ⇒ 节点被换掉、监听留在旧节点上；
   * ③ 回调 ref ⇒ **Radix 是在 React 背后换视口节点的**，React 不会再把 ref 叫一次 ✗）。
   *
   * 判据：**`scroll` 不冒泡，但会走捕获阶段** ⇒ 挂在 `document` 上、按 `event.target`
   * 认"是不是我们那个视口"，就与"视口节点此刻是谁"完全无关 ✓。
   * 处理函数本身稳定，读的都是**当下**那一份（`scrollHandlerRef`）。
   */
  const scrollHandlerRef = useRef<() => void>(() => {})
  scrollHandlerRef.current = () => {
    onTimelineScroll()
    onTimelineScrollRead()
  }
  useEffect(() => {
    const onScroll = () => {
      // ⚠ **不认 `event.target`**（2026-10-16 实测）：视口节点会在 React 背后被换掉，
      //   拿身份比对就可能永远不相等 ⇒ 改成"**任何滚动都问一句**"：
      //   条件（视口在顶部 ＋ 还有更早的分页）由那两个守卫判，代价只是一次读值。
      scrollHandlerRef.current()
    }
    document.addEventListener('scroll', onScroll, { capture: true, passive: true })
    return () => document.removeEventListener('scroll', onScroll, { capture: true })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [roomId])

  /**
   * ★ **滚不动也要读得到**（那颗被撤掉的按钮真正承担的那一件事）。
   *
   * 一屏装得下整页（列表不可滚）却还有更早的分页时，"滚到顶"这个触发**永远不会发生**
   * ⇒ 更早的消息就没有入口了。判据：**列表不可滚 ＋ 还有更早 ⇒ 自动补一页**，
   * 补到"能滚"或"没有更早的了"为止；**上限 5 页**（别把世界问烦的闸，不是功能边界）。
   *
   * 依赖里带 `visibleMessages.length`：补进来一页之后要**再判一次**（这就是那个循环的推进）。
   */
  useEffect(() => {
    const vp = timelineViewport()
    if (!vp || olderLoadingRef.current) return
    if (!useAppStore.getState().nextBeforeByRoom[roomId ?? '']) return
    const notScrollable = vp.scrollHeight <= vp.clientHeight + 8
    if (!notScrollable) return
    if (autoFillRef.current >= AUTO_FILL_MAX_PAGES) return
    autoFillRef.current += 1
    void loadEarlier()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [roomId, visibleMessages.length])

  /** 换房把"自动补页"的计数清零（它是**这一间**的账）。 */
  useEffect(() => {
    autoFillRef.current = 0
  }, [roomId])

  /**
   * **滚到底 ⇒ 这一间就算读过了**（2026-10-05 主人真机反馈）。
   *
   * 今天读游标只在前三处推进：换房、跟随到底部（`shouldFollowToBottom`）、点那颗「有 N 条新消息」。
   * 于是"我翻上去看了几段，新消息来了"这种最普通的处境下，人已经在看那条消息了，未读却一直挂着，
   * 非要用鼠标去会话列表里点一下才消失。这里补上主流那一条：**视口回到（或本来就在）底部 ⇒ 上报已读**。
   *
   * 只在"从不在底部变成在底部"那一刻上报一次（`readOnBottomRef`）—— 滚动是每帧都来的，
   * 每帧发一次请求就是把世界问烦。进房那一刻的初值只记不报（换房那一处已经报过一次）。
   */
  const readOnBottomRef = useRef(false)
  /**
   * **滚到底 ⇒ 这一间就算读过了**（2026-10-05 主人真机反馈）。
   *
   * ⚠ 与上面那条**同一个坑**（2026-10-16）：监听挂在会被换掉的视口节点上 ⇒ 从未生效。
   * 现在同样走**稳定包装 div 上的 `onScroll`**（见 `onTimelineScroll` 头上那段）。
   * 只在"从不在底部变成在底部"那一刻上报一次 —— 滚动是每帧都来的，
   * 每帧发一次请求就是把世界问烦。进房那一刻的初值只记不报（换房那一处已经报过一次）。
   */
  const onTimelineScrollRead = useCallback(() => {
    if (!timelineViewport()) return
    const atBottom = isNearBottom()
    if (atBottom === readOnBottomRef.current) return
    readOnBottomRef.current = atBottom
    if (atBottom && roomId) void markRoomRead(roomId)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [roomId, markRoomRead])

  useEffect(() => {
    // 进房那一刻的初值只记不报（换房那一处已经报过一次）。
    readOnBottomRef.current = isNearBottom()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [roomId, activeRoom?.id])

  useEffect(() => {
    if (roomId && (connection === 'connected' || connection === 'reconnecting')) {
      void loadRoomMessages(roomId)
      // group Room 需要成员列表作为 @ 提及建议源（core 走 members 端点，
      // 连接没有这个能力就不拉取）；直接/direct 房间无提及，不拉取。
      if (activeRoom?.kind === 'group') void refreshRoomMembers(roomId)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [roomId, connection, loadRoomMessages])

  // 主体进行态（在不在 / 正在写）：**拉取式**刷新——没有第二条推送通道
  // （独立 Live WS 已于 2026-09-28 随旧 Live State 删除）。开房期间按
  // `LIVE_REFRESH_MS` 拉一次；离开房间即停（读面可见性仍是该房成员）。
  // 代价如实写在契约里：最多一个刷新周期的延迟。
  //
  // ⚠ 2026-10-10 同一支定时器里多拉一次**单房**（`GET /v1/rooms/{id}`）：
  // 「对方读没读我最后那条」（`peer_read`）住在房间视图里，而**对方读这件事
  // 不产生任何事件**（`read.cursor.advanced` 只投推进者本人）⇒ 光靠事件驱动的
  // `refreshRooms` 会一直停在"未读"，那正好把这一格的意思反过来。单房那一支
  // 窄（一间房一行），与 live 同一节奏。
  useEffect(() => {
    if (!roomId || connection !== 'connected') return
    void refreshRoomLive(roomId)
    void refreshRoom(roomId)
    const timer = window.setInterval(() => {
      void refreshRoomLive(roomId)
      void refreshRoom(roomId)
    }, LIVE_REFRESH_MS)
    return () => window.clearInterval(timer)
  }, [roomId, connection, refreshRoomLive, refreshRoom])

  useEffect(() => {
    lastMessageIdRef.current = null
    if (roomId) void markRoomRead(roomId)
  }, [roomId, markRoomRead])

  const initScrolledRoomRef = useRef<string | null>(null)

  useEffect(() => {
    const last = roomMessages[roomMessages.length - 1]
    if (!last) return
    // 每个房间首次拿到消息后定位到底部（打开会话只滚这一次；
    // 连接是异步的，roomId 变化时消息往往还没到，那时滚会失效）
    if (roomId && initScrolledRoomRef.current !== roomId) {
      initScrolledRoomRef.current = roomId
      lastMessageIdRef.current = last.id
      // ★ **落点判据在纯函数里**（有未读分隔线 ⇒ 落分隔线；没有 ⇒ 落最后一条），
      //   落法对 DOM 说（见 `applyLanding` 头上那段）。
      const plan = landingPlanFor({
        unreadIndex: rows.findIndex((r) => r.unreadStart),
        rowCount: rows.length,
      })
      if (plan) settleLanding(plan)
      return
    }
    if (lastMessageIdRef.current === last.id) return
    lastMessageIdRef.current = last.id
    // 跟随滚动策略（纯逻辑见 selfIdentity.shouldFollowToBottom）：
    // 「我」按当前连接身份（core 真实 Human id）判定，始终滚到底；
    // 对方消息只在已接近底部时跟随，用户向上翻阅旧消息时不被打断。
    if (shouldFollowToBottom(last.authorId, me?.participant?.id, isNearBottom())) {
      void markRoomRead(roomId ?? '')
      scrollToBottom()
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [roomMessages])
  useEffect(() => {
    // 与路由房间同步的惰性房间 id（onLoadReplyTarget 等异步终止条件用）。
    activeRoomIdRef.current = roomId
    setReplyTo(null)
    // 换房清除未读锚点（分隔线只在本次停留期间保留，对齐微信语义）
    unreadAnchorRoomRef.current = null
    setUnreadAnchorId(null)
  }, [roomId])

  // 未读锚点捕获：进房后从 store 快照取一次（markRoomRead 清零前 / 真实消息到达后），
  // 分隔线停留期间不因已读上报而消失。必须声明在换房重置 effect 之后。
  useEffect(() => {
    if (!roomId || unreadAnchorRoomRef.current === roomId) return
    const s = useAppStore.getState()
    const room = s.rooms.find((r) => r.id === roomId)
    const msgs = (s.messagesByRoom[roomId] ?? []).filter((m) => !m.system)
    const count = room?.unreadCount ?? 0
    if (count <= 0 || msgs.length === 0) return
    unreadAnchorRoomRef.current = roomId
    setUnreadAnchorId(msgs.slice(-count)[0]?.id ?? null)
  }, [roomId, roomMessages, activeRoom?.unreadCount])

  const startRoom = (participantIds: string[]) => {
    if (participantIds.length === 1) {
      const byId = allRooms.find((r) => r.id === participantIds[0])
      if (byId) {
        navigate(`/app/chat/${byId.id}`)
        return
      }
      const direct = allRooms.find(
        (r) =>
          r.participantIds.length === 2 &&
          r.participantIds.includes(resolveSelfId(me?.participant?.id)) &&
          r.participantIds.includes(participantIds[0]),
      )
      if (direct) {
        navigate(`/app/chat/${direct.id}`)
        return
      }
      navigate(`/app/chat/${participantIds[0]}`)
      return
    }
    navigate('/app/chat/r-guide')
  }
  const previewImages: PreviewImage[] = roomMessages.flatMap((m) =>
    m.system
      ? []
      : (m.attachments ?? [])
          .filter((a) => a.kind === 'image' && a.url)
          .map((a) => ({ id: a.id, attachment: a, roomId: m.roomId, name: a.name })),
  )
  const jumpToMessage = (messageId: string) => {
    const idx = rows.findIndex((r) => r.message.id === messageId)
    if (idx >= 0) {
      // 行不再是绝对定位（见 `rows` 头上那段）⇒ 跳到某一条就用 DOM 自己的说法，
      // 而且走**共用实现**（有界等待 + 出现即收工 —— 带锚点滚动全仓只许一份）。
      scrollToAnchor(`${MESSAGE_ANCHOR_PREFIX}${messageId}`)
      setFlashId(messageId)
      window.setTimeout(() => setFlashId((cur) => (cur === messageId ? null : cur)), 2000)
    } else {
      const vp = timelineViewport()
      if (vp) vp.scrollTop = 0
    }
  }

  const editLastSelfMessage = () => {
    // ↑ 编辑自己最近一条消息：按真实连接身份判定（core 真实 Human id）。
    // TASK_56：无身份时**不再**回退 fixtures SELF_ID —— 真实世界下那会把一条
    // 假消息认成"我的"。判据收敛在 `resolveSelfId`（真实世界返空 ⇒ 认不出）。
    const selfId = resolveSelfId(me?.participant?.id)
    const last = [...roomMessages].reverse().find((m) => !m.system && m.body && m.authorId === selfId)
    if (!last) return
    setEditRequest({ message: last, seq: Date.now() })
  }

  useEffect(() => {
    const onFocusMessage = (ev: Event) => {
      const detail = (ev as CustomEvent<{ roomId: string; messageId: string }>).detail
      if (!detail) return
      if (detail.roomId !== roomId) {
        navigate(`/app/chat/${detail.roomId}`)
        window.setTimeout(() => {
          window.dispatchEvent(
            new CustomEvent('celira:focus-message', { detail: { roomId: detail.roomId, messageId: detail.messageId } }),
          )
        }, 800)
        return
      }
      jumpToMessage(detail.messageId)
    }
    window.addEventListener('celira:focus-message', onFocusMessage)
    return () => window.removeEventListener('celira:focus-message', onFocusMessage)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [roomId, rows])

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement
      const typing = target.tagName === 'INPUT' || target.tagName === 'TEXTAREA'
      if (e.key === 'ArrowUp' && !typing && roomMessages.length > 0) {
        e.preventDefault()
        editLastSelfMessage()
      }
      if ((e.key === 'k' || e.key === 'K') && (e.ctrlKey || e.metaKey)) {
        e.preventDefault()
        window.dispatchEvent(new CustomEvent('celira:focus-search'))
      }
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [roomMessages])

  const openPreview = (attachmentId: string) => {
    const idx = previewImages.findIndex((a) => a.id === attachmentId)
    if (idx < 0) return
    setPreviewIndex(idx)
    setPreviewOpen(true)
  }

  return (
    <div className="flex h-full min-h-0 w-full overflow-hidden bg-background">
      <div className={cn('h-full min-h-0 w-full md:w-auto md:shrink-0', roomId && 'hidden md:block')}>
        <ConversationList
          rooms={allRooms}
          activeRoomId={activeRoom?.id ?? ''}
          onSelectRoom={(id) => navigate(`/app/chat/${id}`)}
          onCreateRoom={() => setNewRoomOpen(true)}
        />
      </div>
      <main
        className={cn('min-w-0 flex-1 flex-col', roomId ? 'flex' : 'hidden md:flex')}
      >
        {activeRoom ? (
          <>
            <ChatHeader
              room={activeRoom}
              onBack={() => navigate('/app/chat')}
              onOpenInfo={() => setInfoOpen(true)}
              onOpenMembers={() => setInfoOpen(true)}
              onOpenHistory={() => navigate(`/app/chat/${roomId}/history`)}
            />
            {connection === 'reconnecting' && (
              <ConnectionBanner
                state="offline"
                onRetry={() => void retryConnection()}
              />
            )}
            <div
              ref={panelRef}
              className="relative min-h-0 flex-1"
            >
              <ChatWallpaper />
              <ScrollArea
                className="h-full [&_[data-radix-scroll-area-viewport]>div]:!block"
              >
                {/* ★ **顶部那一个转圈**（2026-10-16）：主流形状 —— 没有按钮，
                    读到更早的消息时只在顶上给一行指示（`nextBefore` 那一格本身不上屏：
                    它是个不透明的游标，人不该看见它）。 */}
                {olderLoading && (
                  <div className="flex items-center justify-center gap-2 px-6 pt-4 text-caption text-muted-foreground">
                    <Loader2 className="size-3.5 animate-spin" aria-hidden />
                    正在读更早的消息…
                  </div>
                )}
                {/* 读更早**失败要说出来**（不许静默）＋ 一个明确的重试 —— 主流也是这个形状：
                    顶部一行提示，点一下再试；游标没动，重试就是同一页。 */}
                {olderError && !olderLoading && (
                  <div role="alert" className="flex items-center justify-center gap-2 px-6 pt-4">
                    <p className="text-caption text-presence-failed">没能读到更早的消息：{olderError}</p>
                    <Button variant="outline" size="sm" onClick={() => void loadEarlier()}>
                      重试
                    </Button>
                  </div>
                )}
                {/* ★ **普通文档流**（2026-10-16，见 `rows` 头上那段）：行不再绝对定位、
                    不再有"按量出来的高度摆位"这回事 ⇒ 重叠这一类症状从构造上没有了。 */}
                <div className="min-h-full">
                  {rows.map((row) => {
                    return (
                      <div
                        key={row.key}
                        data-index={row.key}
                        id={`${MESSAGE_ANCHOR_PREFIX}${row.message.id}`}
                      >
                        <div className="mx-auto w-full max-w-5xl 2xl:max-w-6xl px-4 md:px-6">
                          <div className={cn(row.compact ? 'py-1' : 'py-3')}>
                            {row.dayChanged && row.message.day && (
                              <div className="pb-6">
                                <DateDivider label={row.message.day} />
                              </div>
                            )}
                            {row.unreadStart && (
                              <div className="pb-6" id={UNREAD_ANCHOR_ID}>
                                <UnreadDivider />
                              </div>
                            )}
                            {row.message.system ? (
                              <SystemMessage
                                message={row.message}
                                selfId={me?.participant?.id}
                                resolveAuthorName={participantNameOf}
                              />
                            ) : (
                              <MemoizedMessageBubble
                                message={row.message}
                                compact={row.compact}
                                // 自我归属优先用当前连接身份（core 真实 Human id）；
                                // 无连接身份时组件回退 fixtures SELF_ID。
                                selfId={me?.participant?.id}
                                // 作者投影：真实连接用真实 me + room.participants（未知作者
                                // 中性「参与者」），绝不查 fixtures；Mock/Core 回退 fixtures。
                                resolveAuthor={resolveAuthor}
                                // @ 渲染高亮用成员名快照（真实成员投影：agent + human，
                                // 缺省由闸门决定；structured mentions 仍精确覆盖 agent）。
                                mentionMembers={mentionMembers}
                                // 回复/撤回只对已提交消息开放（MessageBubble 内部再按
                                // delivery 裁剪临时 pending）；作者撤回由 selfId 判定。
                                onReply={setReplyTo}
                                onRecall={(m) => roomId && void handleRecall(m)}
                                onRetry={() => void retryMessage(row.message.id)}
                                onPreviewImage={openPreview}
                                onJumpToMessage={jumpToMessage}
                                onForward={(m) => setForwardTargets([m])}
                                // 「删除 · 仅为我」：**本机隐藏**（不是撤回）。
                                onHide={handleHide}
                                // 多选：进多选、勾选/取消。
                                onSelectMode={enterSelectMode}
                                selectionMode={selectMode}
                                selected={isSelected(selected, row.message.id)}
                                onToggleSelect={() =>
                                  setSelected(toggleSelected(selected, row.message.id))
                                }
                                resolveReplyTarget={resolveReplyTarget}
                                onLoadReplyTarget={onLoadReplyTarget}
                                highlighted={row.message.id === flashId}
                                // 「已读 / 未读」只挂在**我最后一条**上（房间级投影，
                                // 不是逐条回执）；其它行 `undefined` ⇒ 那一格不出现。
                                peerRead={
                                  row.message.id === myLastMessageId
                                    ? activeRoom?.peerRead
                                    : undefined
                                }
                              />
                            )}
                          </div>
                        </div>
                      </div>
                    )
                  })}
                  {/* 空态 / 失败态（`chatFeed` 说的是**世界里**的条数）—— 它排在最后一条后面。 */}
                  <div className="mx-auto w-full max-w-5xl 2xl:max-w-6xl space-y-6 px-4 py-6 md:px-6">
                    {chatFeed.kind !== 'messages' && (
                      <div className="px-6 pt-24 text-center">
                        {chatFeed.kind === 'failed' ? (
                          // 红路径不许静默：说人话 + 给重试（读数与文案都来自 `chatFeedState`）。
                          <div role="alert" className="space-y-3">
                            <p className="text-body text-presence-failed">{chatFeed.text}</p>
                            <Button
                              variant="outline"
                              size="sm"
                              onClick={() => roomId && void loadRoomMessages(roomId)}
                            >
                              重新读取
                            </Button>
                          </div>
                        ) : (
                          <p className="text-body text-muted-foreground">{chatFeed.text}</p>
                        )}
                      </div>
                    )}
                    {/*
                      ⚠ `chatFeed` 说的是**世界里**的条数（那是事实）；这里补一句"它们都被你隐藏了"，
                      否则一条都被隐藏时会显示成「这个会话还没有消息」——**那是一句假话**。
                    */}
                    {chatFeed.kind === 'messages' &&
                      visibleMessages.length === 0 &&
                      roomMessages.length > 0 && (
                        <div className="px-6 pt-24 text-center">
                          <p className="text-body text-muted-foreground">
                            这个会话的消息都被你隐藏了（只在你这边）
                          </p>
                        </div>
                      )}
                  </div>
                </div>
              </ScrollArea>
              {activeRoom && activeRoom.unreadCount > 0 && (
                <div className="absolute right-4 bottom-4 z-10">
                  <NewMessagePill
                    count={activeRoom.unreadCount}
                    onClick={() => {
                      void markRoomRead(activeRoom.id)
                      scrollToBottom()
                    }}
                  />
                </div>
              )}
            </div>
            {/* 动作失败条（TASK_72 追补）：撤回失败时**说人话**，不再静默。
                形状照 `MessageInput` 里那条附件失败条（`role="alert"` + presence-failed）。 */}
            {actionError && (
              <p
                role="alert"
                className="mx-4 mt-1.5 text-caption text-presence-failed md:mx-6"
              >
                {actionError}
              </p>
            )}
            {/*
              「已隐藏 · 只在你这边」（2026-10-08）：隐藏是**本机视图**，对面照旧看得见，
              所以不说一句用户会以为"大家都看不见了"。带**撤销**（比弹窗确认更跟手，
              也与"这个仓库不用弹窗做确认"那条一致）。
            */}
            {hideUndo && (
              <p className="mx-4 mt-1.5 flex items-center gap-2 text-caption text-muted-foreground md:mx-6">
                <span>已隐藏 {hideUndo.length} 条 · 只在你这边</span>
                <button
                  type="button"
                  className="cursor-pointer underline underline-offset-2 hover:text-foreground"
                  onClick={() => void undoHide()}
                >
                  撤销
                </button>
              </p>
            )}
            {hideError && (
              <p role="alert" className="mx-4 mt-1.5 text-caption text-presence-failed md:mx-6">
                {hideError}
              </p>
            )}
            {selectNote && (
              <p className="mx-4 mt-1.5 text-caption text-muted-foreground md:mx-6">{selectNote}</p>
            )}
            {/*
              **多选条**（2026-10-08）：进了多选就把输入区**换掉**（主流都是这个形状 ——
              底下那一格此刻的动作是"对这批消息做什么"，不是"再写一条"）。
              批量动作三个：转发 / 复制 / 删除（仅为我）；判据（顺序、跳过空正文）都在
              `lib/messageSelection.ts`，这里只接。
            */}
            {selectMode ? (
              // `role="toolbar"`：这一格是"对这批消息做什么"的一组动作，读屏与真机驱动都按它找。
              <div
                role="toolbar"
                aria-label="多选动作"
                className="flex flex-wrap items-center gap-2 border-t border-border px-4 py-3 md:px-6"
              >
                <span className="text-caption text-muted-foreground">已选 {selected.length} 条</span>
                <Button
                  size="sm"
                  variant="outline"
                  className="gap-1.5"
                  disabled={selected.length === 0}
                  onClick={startBatchForward}
                >
                  <Forward className="size-3.5" />
                  转发
                </Button>
                <Button
                  size="sm"
                  variant="outline"
                  className="gap-1.5"
                  disabled={selected.length === 0}
                  onClick={() => void copySelected()}
                >
                  <Copy className="size-3.5" />
                  复制
                </Button>
                <Button
                  size="sm"
                  variant="outline"
                  className="gap-1.5 text-destructive"
                  disabled={selected.length === 0}
                  onClick={() => void hideSelected()}
                >
                  <Trash2 className="size-3.5" />
                  删除 · 仅为我
                </Button>
                <Button size="sm" variant="ghost" className="ml-auto" onClick={exitSelectMode}>
                  取消
                </Button>
              </div>
            ) : (
              <MessageInput
                room={activeRoom}
                replyTo={replyTo}
                onCancelReply={() => setReplyTo(null)}
                // 回复目标作者投影复用 `resolveAuthor`（与 MessageBubble 同一条解析通路，
                // 不新建第二套）；解析不出时组件自行给中性「参与者」，绝不抛异常、绝不查 fixtures。
                resolveReplyAuthor={resolveAuthor}
                onSend={(input) => {
                  if (roomId) void send(roomId, input)
                }}
                editRequest={editRequest}
              />
            )}
          </>
        ) : (
          <EmptyRoomsState
            onCreateRoom={() => setNewRoomOpen(true)}
            // **一支 Agent 都没有** ⇒ 这一屏改成指路（2026-10-05 走用户视角走出来的第一条摩擦：
            // 新用户第一屏就是这里，而原来那句"从左侧列表挑一个房间"对他毫无意义 —— 他左边是空的）。
            noAgents={ownedAgents.length === 0 && allRooms.length === 0}
          />
        )}
      </main>
      {activeRoom && (
        <RoomInfoDrawer
          room={activeRoom}
          open={infoOpen}
          onOpenChange={setInfoOpen}
          onPreviewImage={openPreview}
        />
      )}
      <NewRoomSheet open={newRoomOpen} onOpenChange={setNewRoomOpen} onStart={startRoom} />
      <ForwardSheet
        open={forwardTargets.length > 0}
        sourceRoomId={roomId ?? ''}
        messages={forwardTargets}
        onOpenChange={(open) => !open && setForwardTargets([])}
      />
      <ImagePreviewLightbox
        open={previewOpen}
        images={previewImages}
        initialIndex={previewIndex}
        onClose={() => setPreviewOpen(false)}
      />
    </div>
  )
}
