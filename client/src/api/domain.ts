/**
 * 前端世界事实的形状（视图模型）。
 *
 * 真实投影与演示数据共用这些形状：它们描述「世界事实长什么样」，与「演示数据从哪来」
 * 无关。因此它不属于 `fixtures/`（那是演示数据），也不属于 `contracts.ts`（那是服务端
 * 线协议的白名单投影）。
 */
export type ParticipantKind = 'human' | 'agent'

export interface Participant {
  id: string
  kind: ParticipantKind
  displayName: string
  initials: string
  official?: boolean
  signature?: string
  since?: string
}

/**
 * 「我的 Agent」列表里的一行：参与者视图 ＋ **这一支还活着吗**。
 *
 * ⚠ `revoked` 是 `/v1/agents` **列表端点自己的事实**（线协议里的 `status`），不是参与者最小
 * 视图的一部分 ⇒ 刻意**不塞进 `toParticipant`**：那个投影是所有参与者共用的，给它加字段会
 * 让房间成员、联系人、消息作者一起多背一个字段，而 `tests/participant-projection.test.ts`
 * 里那条"收刀不改行为"的结构证据正是拿它逐字段比对旧实现的（2026-09-29 记账）。
 * 软撤销后参与者行仍在（契约：不硬删除、含 revoked 历史）⇒ 列表必须说得出这件事，
 * 否则"已撤销"和"活着但没接入"在界面上长得一模一样。
 */
export interface OwnedAgent extends Participant {
  revoked?: boolean
}

/**
 * 「已拉黑」列表里的一行：参与者视图 ＋ **什么时候拉的**。
 *
 * ⚠ 这一格是**单方面**的：`GET /v1/blocks` 只列**我作为 blocker** 的 active 段。
 * 不存在"谁拉黑了我"的读取面（契约明写**拉黑不泄露**：被拦的申请返回与「目标不存在」
 * 同形的 404）⇒ 界面上**不要**把"申请失败"翻译成"被拉黑"，那是客户端自己编的事实。
 */
export interface BlockedPeer extends Participant {
  blockedAt?: string
}

/**
 * 一次关系申请的**回执**（`POST /v1/contacts/requests` 回来的那段事实上限）。
 *
 * ⚠ 为什么必须留住它：服务端在那个响应里已经回答了两件事 —— 「这个 id 是谁」（`peer` 自带
 * `kind` 与名字）和「这条现在在等谁」（`direction`，相对**我**）。客户端从前把整个响应当
 * `void` 丢掉 ⇒ 界面只能对四种完全不同的处境说同一句「请求已发送」：
 *
 * - 刚发出去（201）；对方**早就**发过给我（200 + `incoming`）＝ 该我点头，不是等对方；
 * - 我**之前**发过（200 + `outgoing`）＝ 还在等；已经接受了（200 + `accepted`）＝ 早就是联系人。
 *
 * 把四种说成一种，就是替系统撒谎。
 */
export interface ContactRequestReceipt {
  peer: Participant
  status: 'pending' | 'accepted'
  direction: 'incoming' | 'outgoing'
  replayed: boolean
}

export type AgentActivity = 'idle' | 'thinking' | 'silent' | 'timeout' | 'failed'

export interface Room {
  id: string
  name: string
  avatarLabel?: string
  participantIds: string[]
  participants?: Participant[]
  peer?: Participant
  kind?: 'direct' | 'human_direct' | 'group'
  lastMessagePreview: string
  lastActivityAt: string
  unreadCount: number
  /**
   * 「对方读没读我最后那条」（契约 `CORE_V1_READ_RECEIPT.md` §2）。
   *
   * `undefined` ＝ **问题不成立**（这一格不出现）：房不是 `direct`、对方当前不是
   * 这间房的 active 成员、或我在这间房还没说过话。⚠ 它是**读位置**，不是"对方看到了"
   * ——呈现文案不许写成后者。
   */
  peerRead?: 'read' | 'unread'
  pinned?: boolean
  muted?: boolean
}

export type DeliveryState = 'sending' | 'delivered' | 'failed'

export type AttachmentKind = 'image' | 'file'

export interface Attachment {
  id: string
  kind: AttachmentKind
  name: string
  url?: string
  size?: string
  state?: 'pending' | 'bound' | 'recalled'
  localUrl?: string
  /** Client 展示提示：MIME（audio/* 触发内联播放器）；非 Core 契约字段。 */
  mime?: string
  /** Client 展示提示：录音时长（秒）；非 Core 契约字段。 */
  durationSeconds?: number
}

export interface Message {
  id: string
  roomId: string
  authorId: string
  body: string
  sentAt: string
  day?: string
  delivery: DeliveryState
  replyToId?: string
  mentions?: string[]
  attachments?: Attachment[]
  system?: boolean
  recalledAt?: string
  /**
   * **这一次逻辑发送的 Idempotency-Key**（只存在于本机未送达的那条 pending 上）。
   *
   * 为什么它必须随消息一起留着：契约 `CORE_V1_IM_CONTRACT.md` §6.3 规定
   * 「同 actor + 同 key + 同 Room + 相同 body + 规范 Mention 集合 + 相同
   * `reply_to_message_id` + 完全相同的有序附件 ID 集合 → 返回**原** Message
   * （200，`idempotency_replayed=true`）」。所以重试**复用同一 key** 时，
   * 若首次其实已落库、只是响应丢了，服务端会把原来那条还回来 ——
   * **绝不产生第二条公共消息**（架构红线：重放不等于重新生成）。
   *
   * ⚠ 它**不是公共事实**：服务端的 MessageView 里没有这一格，权威历史读回来的消息
   * 永远不带它。它只活在乐观更新那条 pending 记录上。
   * ⚠ 它**绝不进 URL、绝不进日志或输出**（凭据纪律同款）。
   */
  idempotencyKey?: string
}
