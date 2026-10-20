/**
 * **演示通道的传输层**（mock：fixtures 只读 + 进程内本地覆写）。
 *
 * ## 它是什么，为什么在这里
 *
 * 演示通道（登录页「不连接，先看演示界面」= `setPrefs('core','')`，没有真实凭据）里
 * `store` 拿到的就是本类。于是**整个前端都能在没有 im 服务端、没有家的情况下跑起来**——
 * 这正是"只想看页面长什么样 / 方便调试前端"要的东西。
 *
 * ⚠ 这不是新发明：本仓原本就有 `api/mock.ts`（"fixtures 只读 + 本地覆写：回复模拟、
 * 投递状态流转"），它在某次清理里连同工作树一起没了。**丢了之后演示通道塌了一半**：
 * `store.rooms` 初始是 `[]`、只有 `connect()` 才填，而演示通道没有 token ⇒ 永远不填
 * ⇒ `/app/chat` 恒显示「没有会话」、`/app/agents` 恒显示「0 个」（2026-09-25 实测）。
 * 本文件把那半个补回来，并且**实现的是拆分后的事实域接口**（`RoomApi` /
 * `RelationApi` / `ParticipantApi`；原第四个 `ServiceApi` 已随服务层退役删除）。
 *
 * ## 三条纪律（不许松）
 *
 * 1. **只用 `contracts.ts` / `domain.ts` 的类型**。mock 是**第二份实现**，一旦它自己
 *    发明形状，页面就会"在演示通道能跑、连真 core 就崩"——那是这个方案唯一的真代价，
 *    所以形状必须逐字对着契约写。
 * 2. **只做"够调试用"的本地覆写**，不复制业务逻辑。这里没有权限、没有幂等事务、
 *    没有事件账本——那些是 im 服务端的事，mock **不假装**拥有它们。
 * 3. **不是公共事实**：`readonly mode = 'core'` 是既有类型约束（`ConnectionMode` 只有一个
 *    取值），但本类**一个网络请求都不发**。真实与演示的分野仍然是
 *    `prefs.isRealWorldSession`（手里有没有真实凭据），**不是** `mode`。
 *
 * ## 实现范围（如实登记，别把它读成完整后端）
 *
 * | 方法 | 在这一层的行为 |
 * |---|---|
 * | `me` / `listRooms` / `listMessages` / `loadOlder` | 读 fixtures（+ 本地覆写），**真数据** |
 * | `sendMessage` / `recallMessage` / `advanceReadCursor` | 本地覆写（进程内，刷新即还原），**幂等键按契约语义重放** |
 * | `listEvents` / `subscribeWakeup` | 空页 + 不回调（演示通道没有事件账本；新消息靠本类的 `wakeup` 主动叫一次） |
 * | 联系人 / Agent 所有权 / 绑定 / 凭据 / 服务与呈现 | 读 fixtures，**够页面渲染** |
 * | `fetchAttachmentBlob` / `fetchMemberAvatarBlob` / 头像 PUT-DELETE | **不实现**（要字节通道；演示附件的 `url` 直接指向 `/mock/*.svg`，由 `attachmentSource` 直出） |
 * | 群成员增删 / 转让 / 建房 | 只在本地内存里改，进程退出即还原 |
 */

import type {
  Attachment,
  BlockedPeer,
  ContactRequestReceipt,
  Message,
  Participant,
  Room,
} from '@/api/domain'
import {
  type BackendBindingView,
  type CredentialView,
  type GetAgentResponse,
  type GetMemberPresentationsResponse,
  type GetPresentationResponse,
  type GetProfileResponse,
  type GetRoomResponse,
  type HomeView,
  type MeView,
  type PutPresentationResponse,
  type PutProfileResponse,
  type RoomMembersItem,
  type PairingGrantIssue,
  type PairingGrantListEnvelope,} from './contracts'
import type {
  AppEvent,
  ParticipantApi,
  RelationApi,
  RoomApi,
  RoomListEntry,
  HomeApi,} from './types'
import { ApiError } from './client'
import {
  SELF_ID,
  agentOwnerships,
  backendBindings,
  backendCredentials,
  messages as fixtureMessages,
  participants,
  rooms as fixtureRooms,
} from '@/fixtures/data'

/** 演示世界的假 home_id（`GET /v1/home` 在演示通道的形状）。 */
const DEMO_HOME_ID = 'demo-home-0001'

/**
 * 演示通道里"很久以前"的那个固定时刻。
 *
 * 为什么不用 `new Date()`：演示数据每次渲染都换时间的话，截图视觉回归会**永远在变**，
 * 而那变化毫无信息量。演示通道的时间要么来自 fixture（`2026-08-02` 这种），
 * 要么来自这一个常量 —— **不来自"现在"**。
 */
const DEMO_EPOCH = '2026-01-01T00:00:00.000Z'

/** 演示里的"现在"：iso 时间戳固定成一个值，读数才不会每次刷新都在跳。 */
const NOW = '2026-09-25T12:00:00.000Z'

function participantOf(id: string): Participant | undefined {
  return participants.find((p) => p.id === id)
}

/** `domain.Participant` → 契约的参与者最小视图（字段名照抄服务端 snake_case）。 */
function toMinView(p: Participant) {
  return { id: p.id, kind: p.kind, display_name: p.displayName, status: 'active' }
}

export class MockApi implements RoomApi, RelationApi, ParticipantApi, HomeApi {
  readonly mode = 'core' as const
  onUpdate?: () => void
  onConnectionChange?: (state: 'online' | 'offline') => void

  /** 进程内的可变状态：由 fixtures 播种，刷新页面即还原（演示通道没有持久化）。 */
  private roomList: Room[] = structuredClone(fixtureRooms)
  private msgList: Message[] = structuredClone(fixtureMessages)
  /** 额外加进来的联系人（`sendContactRequest` 的结果）。 */
  private extraContacts: Participant[] = []
  /** 已接受的联系人 id（`acceptContact` 的结果）。 */
  private acceptedIds = new Set<string>(
    fixtureRooms.filter((r) => r.kind === 'human_direct').flatMap((r) => r.participantIds.filter((id) => id !== SELF_ID)),
  )
  /**
   * 我拉黑过的主体（`blockParticipant` / `unblockParticipant` 的结果）。
   *
   * 用 `Map` 而不是 `Set`：列表要按对端渲染一行参与者，而 `Set` 只剩 id
   * （演示通道没有"按 id 反查参与者"的权威面——`participantOf` 只认 fixtures 里的那些）。
   */
  private blockedIds = new Map<string, Participant>()
  /** 置顶 / 免打扰**不在这里**：它们是用户侧偏好，归 `api/roomPrefs.ts`（经 `api/storage.ts` 窄口落本机）。 */
  private seq = 0
  private wakeupHandler?: () => void
  private timers = new Set<ReturnType<typeof setTimeout>>()

  // ── 连接与身份 ────────────────────────────────────────────────────────────

  /**
   * 演示身份的返回值。
   *
   * ⚠ **只给线协议真有的那几格**（`MeView` 的形状）：`Participant` 上的 `signature` /
   * `since` / `official` 是**前端视图模型的字段**，线协议这一侧根本没有它们
   * （`participantProjection.ts` 头部就是这么写的，投影时给空串）。
   * 演示想显示那句签名，走的是 `participantDirectory` 的 fixtures 兜底查询链，
   * **不是**从 `me()` 里塞回来 —— 塞回来等于让 mock 发明一个契约字段。
   */
  async me(): Promise<MeView> {
    const self = participantOf(SELF_ID)
    return {
      participant: {
        id: SELF_ID,
        kind: 'human',
        display_name: self?.displayName ?? '我',
        status: 'active',
        created_at: NOW,
      },
      session: { id: 'demo-session', created_at: NOW },
      request_id: 'demo-req',
    }
  }

  async worldIdentity(): Promise<HomeView> {
    return { home_id: DEMO_HOME_ID } as HomeView
  }

  // ── 房间与消息 ────────────────────────────────────────────────────────────

  async listRooms(): Promise<RoomListEntry[]> {
    return this.roomList.map((room) => {
      // 对端：私聊/直聊取另一个成员；群聊没有单一对端（与 core 的 `peer: null` 同义）。
      const otherIds = room.participantIds.filter((id) => id !== SELF_ID)
      const peerId = room.kind === 'group' || otherIds.length !== 1 ? null : otherIds[0]
      const peer = peerId ? participantOf(peerId) : undefined
      return { room: { ...room }, peer: peer ? [peer] : [] }
    })
  }

  private messagesOf(roomId: string): Message[] {
    return this.msgList.filter((m) => m.roomId === roomId)
  }

  async listMessages(roomId: string): Promise<{ messages: Message[]; nextBefore: string | null }> {
    // 演示通道没有分页语义：一屏全给，永远没有"更早的"。
    return { messages: this.messagesOf(roomId).map((m) => ({ ...m })), nextBefore: null }
  }

  async loadOlder(): Promise<{ messages: Message[]; nextBefore: string | null }> {
    return { messages: [], nextBefore: null }
  }

  /**
   * 发一条消息。
   *
   * **幂等按契约语义做**（§6.3）：同 key 再打一次 ⇒ 返回**同一条** id，
   * `idempotency_replayed = true`。演示通道里这条不是装饰：`store.retry` 就靠它
   * 验证"重试不会多出第二条消息"。
   */
  async sendMessage(
    roomId: string,
    input: {
      body?: string
      replyToId?: string
      attachments?: Attachment[]
      mentions?: string[]
      authorId: string
      idempotencyKey?: string
    },
  ): Promise<{ messageId: string; message?: Message; idempotencyReplayed?: boolean }> {
    const key = input.idempotencyKey ?? `demo-${++this.seq}`
    // 幂等重放：这次逻辑发送已经落过一条 ⇒ 还它原来那条。
    const seen = this.msgList.find((m) => m.idempotencyKey === key)
    if (seen) return { messageId: seen.id, message: { ...seen }, idempotencyReplayed: true }

    const message: Message = {
      id: `demo-m-${++this.seq}`,
      roomId,
      authorId: input.authorId,
      body: input.body ?? '',
      sentAt: '刚刚',
      day: '今天',
      delivery: 'delivered',
      replyToId: input.replyToId,
      mentions: input.mentions,
      attachments: input.attachments,
      idempotencyKey: key,
    }
    this.msgList = [...this.msgList, message]
    this.touchRoom(roomId, message)
    this.scheduleAgentReply(roomId, message)
    return { messageId: message.id, message: { ...message }, idempotencyReplayed: false }
  }

  async recallMessage(roomId: string, messageId: string): Promise<void> {
    this.msgList = this.msgList.map((m) =>
      m.roomId === roomId && m.id === messageId
        ? { ...m, system: true, body: '', recalledAt: new Date().toISOString() }
        : m,
    )
    this.wakeup()
  }

  async advanceReadCursor(roomId: string, messageId: string): Promise<void> {
    void messageId
    this.roomList = this.roomList.map((r) => (r.id === roomId ? { ...r, unreadCount: 0 } : r))
  }

  async markRoomRead(roomId: string): Promise<void> {
    await this.advanceReadCursor(roomId, '')
  }

  async listEvents(after?: string): Promise<{ events: AppEvent[]; nextAfter: string }> {
    // 演示通道**没有事件账本**：新消息不走这里（走 `wakeup` 让 store 重读房间）。
    // 返回空页而不是编造事件 —— 编造出来的 `member.joined` 之类会在时间线里
    // 留下演示数据本身都没有的系统消息。
    return { events: [], nextAfter: after ?? '' }
  }

  /**
   * 无事实唤醒的回调登记。
   *
   * core 线这条通往 WS；演示通道没有服务端，所以本类只在**自己改了数据之后**
   * 主动叫一次（`wakeup()`）—— 效果一样：store 收到就重读权威历史。
   */
  subscribeWakeup(handler: () => void): () => void {
    this.wakeupHandler = handler
    return () => {
      this.wakeupHandler = undefined
      for (const timer of this.timers) clearTimeout(timer)
      this.timers.clear()
    }
  }

  private wakeup(): void {
    this.wakeupHandler?.()
  }

  private touchRoom(roomId: string, latest: Message): void {
    this.roomList = this.roomList.map((r) =>
      r.id === roomId
        ? { ...r, lastMessagePreview: latest.body.slice(0, 48) || '[附件]', lastActivityAt: '刚刚' }
        : r,
    )
  }

  /**
   * 「Agent 回一句」的演示模拟：延迟一轮，把一句 canned 回复放进该房间。
   *
   * **canned 是刻意的**：演示通道不该假装自己会生成内容（那正是"客户端不生成 Agent 内容"
   * 这条红线要防的观感）。它只让人看见"Agent 回了一句"这条链路是通的——消息落进房间、
   * store 被唤醒、时间线重读。
   */
  private scheduleAgentReply(roomId: string, trigger: Message): void {
    const room = this.roomList.find((r) => r.id === roomId)
    if (!room || room.kind === 'human_direct') return
    const agentId = room.participantIds.find((id) => participantOf(id)?.kind === 'agent')
    if (!agentId) return
    // 自己就是 Agent 发的 ⇒ 不自我回复（避免两个演示 Agent 互相刷屏）。
    if (trigger.authorId === agentId) return
    const timer = setTimeout(() => {
      this.timers.delete(timer)
      const reply: Message = {
        id: `demo-m-${++this.seq}`,
        roomId,
        authorId: agentId,
        body: '（演示通道的占位回复：真实世界里这句由 Agent 的执行端生成，客户端不生成内容。）',
        sentAt: '刚刚',
        day: '今天',
        delivery: 'delivered',
      }
      this.msgList = [...this.msgList, reply]
      this.touchRoom(roomId, reply)
      this.wakeup()
    }, 700)
    this.timers.add(timer)
  }

  // ── 房间成员与呈现（够房间详情抽屉渲染）──────────────────────────────────

  async getRoom(roomId: string): Promise<GetRoomResponse> {
    const room = this.roomList.find((r) => r.id === roomId)
    return {
      room: {
        id: roomId,
        kind: room?.kind ?? 'direct',
        status: 'active',
        name: room?.name ?? null,
        created_at: NOW,
      },
      request_id: 'demo-req',
    } as GetRoomResponse
  }

  async listRoomMembers(roomId: string): Promise<RoomMembersItem[]> {
    const room = this.roomList.find((r) => r.id === roomId)
    if (!room) return []
    return room.participantIds.map((id, index) => {
      const p = participantOf(id)
      return {
        membership: {
          membership_id: `demo-mb-${roomId}-${id}`,
          participant_id: id,
          role: index === 0 ? ('owner' as const) : ('member' as const),
          joined_at: NOW,
          left_at: null,
          closed_by: null,
        },
        participant: toMinView(p ?? { id, kind: 'human', displayName: id, initials: id.slice(0, 1) }),
      }
    })
  }

  async getMemberPresentations(roomId: string): Promise<GetMemberPresentationsResponse> {
    // 演示通道不做群内呈现（那是 Group Room Presentation 的公共事实）：**如实返回空**
    // 而不是编一批假的展示名 —— 编出来的名字会被当成"这个 Agent 在群里的称呼"。
    void roomId
    return { members: [], request_id: 'demo-req' } as GetMemberPresentationsResponse
  }

  async getRoomPresentation(roomId: string, participantId: string): Promise<GetPresentationResponse> {
    void roomId
    return { presentation: null, participant_id: participantId, request_id: 'demo-req' } as unknown as GetPresentationResponse
  }

  async putRoomPresentation(): Promise<PutPresentationResponse> {
    // 演示通道不落任何持久事实：写路径如实报"这一层没有它"。
    throw new Error('demo channel: 群内呈现的写路径不开放')
  }

  // ── 联系人 / Agent 所有权 / 服务 ──────────────────────────────────────────

  async listContacts(): Promise<{ accepted: Participant[]; pending: Participant[] }> {
    const humans = this.roomList
      .filter((r) => r.kind === 'human_direct')
      .flatMap((r) => r.participantIds.filter((id) => id !== SELF_ID))
    const accepted = [...new Set([...humans, ...this.acceptedIds])]
      .map(participantOf)
      .filter((p): p is Participant => p !== undefined)
    const pending = this.extraContacts.filter((p) => !this.acceptedIds.has(p.id))
    return { accepted, pending }
  }

  /**
   * 演示通道的"发一段关系申请"：目标必须在夹具里，否则与真机同形地 404
   * （`participant_not_found`）—— 这一格从前**静默成功**，页面于是对着一串瞎贴的 id
   * 说「请求已发送」，那是演示通道替真机撒谎。
   *
   * ⚠ 发出去的那一半**不进** `extraContacts`：那份读数是「**等我回答**」（`direction ===
   * 'incoming'`，与 `core.ts` 的 `listContacts` 同一判据）。真实世界里 outgoing 的那条
   * 不在任何列表里，唯一的反馈就是这张回执 —— 演示通道跟着这条走，不另造一份。
   */
  async sendContactRequest(participantId: string): Promise<ContactRequestReceipt> {
    const target = participantOf(participantId)
    if (!target) throw new ApiError(404, 'participant_not_found')
    return {
      peer: target,
      status: 'pending',
      direction: 'outgoing',
      replayed: false,
    }
  }

  async acceptContact(humanId: string): Promise<void> {
    this.acceptedIds.add(humanId)
  }

  /**
   * 拒绝给我的那条申请：演示通道只把它**从待确认里拿掉**。
   *
   * ⚠ 演示通道**不模拟**契约里那半个语义（段以 `declined` 闭合、对端一无所知）——它没有
   * 周期模型。这里只保证"按钮点了有反应、那一条不再等你答"，并在本文件顶部那句"演示通道
   * 不落任何持久事实"的既有纪律之内。真实行为以 core 分支为准。
   */
  async declineContact(participantId: string): Promise<void> {
    this.extraContacts = this.extraContacts.filter((p) => p.id !== participantId)
  }

  /**
   * 断交：演示通道里等价于"不再是联系人"。
   *
   * ⚠ 契约里它还会**关闭双方在那个 human_direct 房间的成员周期**（房间从双方列表消失、
   * 永久封存）。演示通道的房间是夹具，**不跟着关**——所以演示通道上"断交"看起来比真机轻。
   * 这是演示通道的已知差距，不是产品语义：界面上那段不可逆说明照原样显示。
   */
  async removeContact(participantId: string): Promise<void> {
    this.acceptedIds.delete(participantId)
    this.extraContacts = this.extraContacts.filter((p) => p.id !== participantId)
  }

  /** 拉黑：契约里 **拉黑 ⊇ 解除**，所以这里也先断关系再进黑名单。 */
  async blockParticipant(participantId: string): Promise<void> {
    await this.removeContact(participantId)
    const target = participantOf(participantId)
    if (target) this.blockedIds.set(target.id, target)
  }

  /** 解除拉黑。契约明写**不恢复关系** ⇒ 这里也只把它从黑名单里拿掉。 */
  async unblockParticipant(participantId: string): Promise<void> {
    this.blockedIds.delete(participantId)
  }

  async listBlocks(): Promise<BlockedPeer[]> {
    return [...this.blockedIds.values()]
  }

  async listOwnedAgents(): Promise<Participant[]> {
    const ids = agentOwnerships.filter((o) => o.ownerId === SELF_ID).map((o) => o.agentId)
    return ids.map(participantOf).filter((p): p is Participant => p !== undefined)
  }

  async getAgent(agentId: string): Promise<GetAgentResponse> {
    const p = participantOf(agentId) ?? { id: agentId, displayName: agentId }
    return {
      agent: {
        id: agentId,
        kind: 'agent',
        display_name: p.displayName,
        status: 'active',
        created_at: NOW,
        revoked_at: null,
      },
      ownership: {
        agent_id: agentId,
        human_id: SELF_ID,
        status: 'active',
        created_at: NOW,
        revoked_at: null,
      },
      request_id: 'demo-req',
    }
  }

  async getAgentProfile(agentId: string): Promise<GetProfileResponse> {
    const p = participantOf(agentId)
    const agent = (await this.getAgent(agentId)).agent
    const ownership = (await this.getAgent(agentId)).ownership
    return {
      agent,
      ownership,
      profile: {
        participant_id: agentId,
        profile_version: 1,
        // `signature` 是**前端视图模型**的字段（线协议没有它）：演示通道拿它当
        // "一句话介绍"的演示来源，这是 fixtures 数据自身的语义，不是契约字段。
        short_description: p?.signature ?? null,
        public_description: null,
        avatar: null,
        published_by_id: SELF_ID,
        published_by_kind: 'human',
        published_at: NOW,
      },
      request_id: 'demo-req',
    }
  }

  async putAgentProfile(agentId: string, input: {
    baseVersion: number
    shortDescription: string | null
    publicDescription: string | null
  }): Promise<PutProfileResponse> {
    return {
      profile: {
        participant_id: agentId,
        profile_version: input.baseVersion + 1,
        short_description: input.shortDescription,
        public_description: input.publicDescription,
        avatar: null,
        published_by_id: SELF_ID,
        published_by_kind: 'human',
        published_at: new Date().toISOString(),
      },
      idempotency_replayed: false,
      request_id: 'demo-req',
    }
  }

  async listBackendBindings(agentId: string): Promise<BackendBindingView[]> {
    // ⚠ **不许 `as unknown as` 硬转**（2026-09-25 修）：`fixtures/data.ts` 的
    // `backendBindings` 是**演示目录的形状**（`agentId` / `backendLabel` / `state` / `scopes`），
    // 不是契约形状（`id` / `agent_id` / `label` / `status` / `created_at` / `revoked_at`）。
    // 硬转能让类型检查闭嘴，代价是**运行时字段全是 `undefined`** —— 实测后果：
    // 列表里每一行的 `key` 都是 `undefined`（React 报 key 重复），
    // 而 `status === 'active'` 恒假 ⇒ 界面把一份**生效中**的绑定画成"已失效"。
    // 演示通道的数据也要**如实**，所以这里老老实实映射一遍，并**按 agent 过滤**
    // （契约的 `listBackendBindings(agent_id)` 本来只回那一个 Agent 的绑定）。
    return backendBindings
      .filter((b) => b.agentId === agentId)
      .map((b, index) => ({
        id: `demo-binding-${b.agentId}-${index + 1}`,
        agent_id: b.agentId,
        created_by_human_id: SELF_ID,
        label: b.backendLabel,
        status: b.state === 'connected' ? 'active' : 'revoked',
        created_at: DEMO_EPOCH,
        revoked_at: b.state === 'connected' ? null : DEMO_EPOCH,
      }))
  }

  async listCredentials(agentId: string, bindingId: string): Promise<CredentialView[]> {
    // 同一条纪律（见上）：fixture 的 `{state, issuedAt}` → 契约的 `{status, created_at}`，
    // 并**按绑定过滤**（契约的 `listCredentials(agent_id, binding_id)` 只回那条绑定的凭据）。
    void agentId
    return backendCredentials
      .filter((cred) => `demo-binding-${cred.agentId}-1` === bindingId)
      .map((cred) => ({
        id: cred.id,
        binding_id: bindingId,
        status: cred.state === 'active' ? 'active' : 'revoked',
        created_at: `${cred.issuedAt}T00:00:00.000Z`,
        revoked_at: cred.state === 'active' ? null : `${cred.issuedAt}T00:00:00.000Z`,
      }))
  }

  // ── 建房（都在本地内存里改，进程退出即还原）──────────────────────────────

  private addRoom(room: Room): string {
    this.roomList = [...this.roomList, room]
    return room.id
  }

  private newRoomId(prefix: string): string {
    return `demo-r-${prefix}-${++this.seq}`
  }

  /**
   * 与该 Agent 的那一间私人房。**演示通道是 Core 新规则的镜像**：一间房 = 一段
   * 关系——同一对 Human–Agent 只有一间，已经有了就回放同一间（不再新建）。
   */
  async createDirectRoom(agentId: string): Promise<{ roomId: string }> {
    const existing = this.roomList.find(
      (r) =>
        r.kind === 'direct' &&
        r.participantIds.length === 2 &&
        r.participantIds.includes(SELF_ID) &&
        r.participantIds.includes(agentId),
    )
    if (existing) return { roomId: existing.id }
    const p = participantOf(agentId)
    const roomId = this.newRoomId('direct')
    this.addRoom({
      id: roomId,
      name: p?.displayName ?? '新会话',
      participantIds: [SELF_ID, agentId],
      lastMessagePreview: '',
      lastActivityAt: '刚刚',
      unreadCount: 0,
      kind: 'direct',
    })
    return { roomId }
  }

  async createHumanDirectRoom(humanId: string): Promise<{ roomId: string }> {
    const p = participantOf(humanId)
    const roomId = this.newRoomId('human')
    this.addRoom({
      id: roomId,
      name: p?.displayName ?? '新会话',
      participantIds: [SELF_ID, humanId],
      lastMessagePreview: '',
      lastActivityAt: '刚刚',
      unreadCount: 0,
      kind: 'human_direct',
    })
    return { roomId }
  }

  async createGroup(name: string): Promise<{ roomId: string }> {
    const roomId = this.newRoomId('group')
    this.addRoom({
      id: roomId,
      name,
      avatarLabel: name.slice(0, 1),
      participantIds: [SELF_ID],
      lastMessagePreview: '',
      lastActivityAt: '刚刚',
      unreadCount: 0,
      kind: 'group',
    })
    return { roomId }
  }

  async createAgent(displayName: string): Promise<{ agentId: string }> {
    const agentId = `demo-a-${++this.seq}`
    participants.push({ id: agentId, kind: 'agent', displayName, initials: displayName.slice(0, 1) })
    agentOwnerships.push({ agentId, ownerId: SELF_ID, createdAt: NOW.slice(0, 10) } as (typeof agentOwnerships)[number])
    return { agentId }
  }

  /**
   * 演示通道的邀请：**不联网**，给一个当场就能看的假码（形状与真码一致），
   * 让陈列室与未连接态也能渲染这一格。真实世界走 `CoreApi`。
   */
  async issueInvite(): Promise<PairingGrantIssue> {
    return {
      grant_id: 'demo-grant',
      status: 'active',
      grants: 'new_human',
      device_label: null,
      issued_at: new Date().toISOString(),
      expires_at: new Date(Date.now() + 72 * 3600 * 1000).toISOString(),
      token: 'clr_pg_demo_invite_code_not_a_real_credential00',
    }
  }

  async listGrants(): Promise<PairingGrantListEnvelope> {
    return { grants: [] }
  }

  async revokeGrant(_grantId: string): Promise<void> {}

  async inviteMember(roomId: string, participantId: string): Promise<void> {
    this.roomList = this.roomList.map((r) =>
      r.id === roomId && !r.participantIds.includes(participantId)
        ? { ...r, participantIds: [...r.participantIds, participantId] }
        : r,
    )
  }

  async removeMember(roomId: string, participantId: string): Promise<void> {
    this.roomList = this.roomList.map((r) =>
      r.id === roomId
        ? {
            ...r,
            participantIds: r.participantIds.filter((id) => id !== participantId),
          }
        : r,
    )
  }

  async leaveRoom(roomId: string): Promise<void> {
    this.roomList = this.roomList.filter((r) => r.id !== roomId)
  }

  async transferOwnership(roomId: string, participantId: string): Promise<void> {
    await this.inviteMember(roomId, participantId)
  }

  async revokeAgent(agentId: string): Promise<void> {
    this.roomList = this.roomList.filter((r) => !r.participantIds.includes(agentId))
  }

  async revokeSession(): Promise<void> {
    // 演示通道没有会话事实可撤销：它**不断开任何东西**（`store.disconnect()` 才是清场的那只手）。
  }

  async uploadAttachment(): Promise<{ attachmentId: string }> {
    // 演示附件的 `url` 直指 `/mock/*.svg`（`lib/attachmentSource` 直出），不需要上传通道。
    throw new Error('demo channel: 附件上传不开放（演示附件是本地静态资源）')
  }
}
