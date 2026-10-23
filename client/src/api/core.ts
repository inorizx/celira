// **值导入必须带 `.ts` 扩展名**：core.ts 会被 `node --test --experimental-strip-types`
// 直接加载（测试 import 它），而 Node 的 ESM 解析不认无扩展名的相对路径。
// `allowImportingTsExtensions: true` 已开着，所以两边都接受。
// （全是 `import type` 的模块没这个问题——类型剥离时被整条去掉。）
import { fetchBlob, fetchJson, joinBaseUrl, newIdempotencyKey, uploadMultipart, webSocketBase } from './client.ts'
import type {
  AttachmentView,
  BackendBindingView,
  CallEnvelope,
  CallInitiateRequest,
  CallInitiateResponse,
  CallSignalRequest,
  CallStreamEvent,
  CreateBindingResponse,
  CreateContactResponse,
  CreateRoomResponse,
  CredentialView,
  GetAgentResponse,
  GetMemberPresentationsResponse,
  GetPresentationResponse,
  GetProfileResponse,
  GetRoomResponse,
  HomeView,
  HumanEventEnvelope,
  HumanEventsEnvelope,
  InviteMemberResponse,
  IssueCredentialResponse,
  ListBindingsResponse,
  ListBlocksResponse,
  ListContactsResponse,
  ListCredentialsResponse,
  MeView,
  MessageHistoryEnvelope,
  MessageView,
  RecallResponse,
  RoomListEnvelope,
  RoomMembersEnvelope,
  RoomLiveSnapshot,
  RoomMembersItem,
  SendMessageResponse,
  TransferOwnerResponse,
  UploadAttachmentResponse,
  PutAttentionResponse,
  PutPresentationResponse,
  PutProfileResponse,
  RoomListEntryView,
  GetMyProfileResponse,
  PairingGrantIssue,
  PairingGrantListEnvelope,
} from './contracts'
import type {
  AppEvent,
  CallApi,
  ParticipantApi,
  RelationApi,
  RoomApi,
  RoomListEntry,
  HomeApi,
} from './types'
import type {
  Attachment,
  BlockedPeer,
  ContactRequestReceipt,
  Message,
  OwnedAgent,
  Participant,
  Room,
  RoomAttention,
} from '@/api/domain'
import { agentProfilePath, buildAgentProfilePutRequest } from './profile.ts'
import {
  buildRoomPresentationPutRequest,
  memberPresentationsPath,
  roomPresentationPath,
} from './presentation.ts'
// 提醒档的宽容解析（缺行 / 缺字段 / 读不懂 ⇒ `all`）：`PUT`/`GET` 两侧共用同一份判据。
import { parseRoomAttention } from './roomAttention.ts'
// 参与者投影的**唯一权威**（原先本文件与 store.ts 各有一份逐字相同的副本）。
import { toParticipant } from './participantProjection.ts'
// Call-stream 帧的解析（纯函数、fail closed）：认不出的帧一律丢掉，绝不猜。
import { parseCallStreamFrame } from './callFrames.ts'

function toAttachment(a: AttachmentView, roomId: string): Attachment {
  return {
    id: a.id,
    kind: a.content_type.startsWith('image/') ? 'image' : 'file',
    name: a.file_name,
    size: a.size_bytes > 0 ? `${Math.max(1, Math.round(a.size_bytes / 1024))} KB` : undefined,
    url: `/v1/rooms/${roomId}/attachments/${a.id}`,
    mime: a.content_type,
  }
}

function dayLabel(iso: string): string {
  const d = new Date(iso)
  const now = new Date()
  const startOf = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime()
  const diff = startOf(now) - startOf(d)
  if (diff === 0) return '今天'
  if (diff === 86400000) return '昨天'
  if (diff > 0 && diff < 7 * 86400000) {
    return ['周日', '周一', '周二', '周三', '周四', '周五', '周六'][d.getDay()]
  }
  return `${d.getMonth() + 1} 月 ${d.getDate()} 日`
}

function timeLabel(iso: string): string {
  const d = new Date(iso)
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${pad(d.getHours())}:${pad(d.getMinutes())}`
}

/**
 * **线协议 MessageView → 视图模型 `Message` 的唯一投影点。**
 *
 * 导出是为了让"撤回是终态 tombstone"这条配方能被 `node --test` 直接钉住
 * （`tests/message-projection.test.ts`）——它此前**一条测试都没有**，
 * 于是"撤回消息仍渲染成气泡"这个缺陷活了下来（2026-09-26 审计）。
 */
export function toMessage(v: MessageView): Message {
  // **撤回是终态 tombstone，不是气泡**（`DESIGN_SYSTEM.md` 的核心交互配方）。
  // ⚠ 2026-09-26 修：这一格原先**从来不置 `system`** ⇒ 权威历史里的撤回消息
  // 一路进 `MessageBubble`（有气泡、头像可点、右键可回复、可再撤回），
  // 而事件路径置上的 `system: true`（`store.ts` 的 `message.recalled` 那一支）
  // 会被下一次唤醒的**权威重读整体覆盖** ⇒ 撤回在刷新后又变回气泡。
  // 两条路必须给**同一个形状**，所以判据落在这一处（唯一投影点）。
  const recalled = v.recalled_at != null
  return {
    id: v.id,
    roomId: v.room_id,
    authorId: v.actor.id,
    body: v.body ?? '',
    sentAt: timeLabel(v.created_at),
    day: dayLabel(v.created_at),
    delivery: 'delivered',
    replyToId: v.reply_to_message_id ?? undefined,
    mentions: v.mentions,
    attachments: v.attachments.map((a) => toAttachment(a, v.room_id)),
    system: recalled || undefined,
    recalledAt: v.recalled_at ?? undefined,
  }
}

/**
 * **线协议房间行 → 视图模型 `RoomListEntry` 的唯一投影点**（2026-10-23 抽出）。
 *
 * 为什么导出（与 `toMessage` 同因）：`node --test` 是 TS 类型剥离模式、没有 jsdom，
 * "缺字段怎么读"这种判据只能在**纯函数**上量（`tests/room-attention.test.ts`）。
 *
 * **提醒档（`attention`）就在这一处**（契约 `CORE_V1_IM_CONTRACT.md` §9.1）：
 * group 房服务端恒给 `all` / `mentions`；两人房与旧服务端可能省略字段 —— 缺字段 / `null` /
 * 读不懂一律按 `all` 读（"缺行 = all"）。反方向（猜成 `mentions`）会**静默吞掉**本该提醒的
 * 群消息，猜错只会多响一声。
 */
export function toRoomListEntry(v: RoomListEntryView): RoomListEntry {
  const peer = v.peer ? [toParticipant(v.peer)] : []
  const room: Room = {
    id: v.room.id,
    name: v.room.name ?? peer[0]?.displayName ?? '会话',
    avatarLabel: v.room.name?.slice(0, 1),
    participantIds: peer.map((p) => p.id),
    participants: [...peer],
    peer: peer[0],
    kind: (v.room.kind as Room['kind']) ?? undefined,
    lastMessagePreview: '',
    lastActivityAt: '',
    unreadCount: v.read_state.unread_count,
    // 「对方读没读我最后那条」：**原样搬**（服务端只在问题成立时给这一格；
    // 缺字段 / null 都表示"这一格不出现"，不许就近映射成 'unread'）。
    peerRead: v.peer_read ?? undefined,
    // 提醒档：唯一的宽容解析点（缺省 `all`）。`Room.attention` 恒有值，
    // 于是门控（`api/notificationGate.ts`）不必再判 "undefined 该当什么"。
    attention: parseRoomAttention(v.attention),
  }
  return { room, peer }
}

/**
 * **世界侧传输层的唯一实现**（2026-09-25 拆分刀）。
 *
 * 四个 `implements` 对应四个事实域（房间与消息 / 关系 / 参与者管理 / Agent 服务），
 * 全部是 im 服务端事实。原先这里写的是 `implements ChatApi`——那个 62 方法的上帝接口
 * 已经拆开，见 `types.ts` 头部的裁决。
 *
 * ⚠ **家平面不在这个类里**：`homes.ts` / `homeStatus.ts` / `homeConfig.ts` 走自己的
 * `fetch`＋家钥匙，不经 `getApi()`。所以"Agent 住哪台机器、醒过几次、用哪条模型"这些问题
 * 永远不该从这个类上找答案。
 */
export class CoreApi implements RoomApi, RelationApi, ParticipantApi, HomeApi, CallApi {
  readonly mode = 'core' as const
  onUpdate?: () => void
  onConnectionChange?: (state: 'online' | 'offline') => void
  private token: string
  /** 服务器地址；空串 = 同源（页面自身来源的 `/v1`）。 */
  private serverUrl: string
  private ws: WebSocket | null = null
  private wakeupHandler?: () => void
  private reconnectAttempt = 0
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null
  /** Call-stream（`/v1/call/stream`）：**另一条流**，自己的连接与退避，与唤醒流互不影响。 */
  private callStream: WebSocket | null = null
  private callStreamHandler?: (frame: CallStreamEvent) => void
  private callStreamResync?: () => void
  private callStreamAttempt = 0
  private callStreamTimer: ReturnType<typeof setTimeout> | null = null

  constructor(token: string, serverUrl = '') {
    this.token = token
    this.serverUrl = serverUrl
  }

  private api<T>(path: string, opts?: Parameters<typeof fetchJson<T>>[1]): Promise<T> {
    return fetchJson<T>(path, { ...opts, token: this.token, baseUrl: this.serverUrl })
  }

  /**
   * 本连接的世界端点：给了服务器地址就用它（直连任意 core），否则同源
   * `ws(s)://<location.host>`（家里托管的页面走既有转发）。
   */
  private wsUrl(path: string): string {
    const base = webSocketBase(this.serverUrl)
    if (base) return joinBaseUrl(base, path)
    const proto = location.protocol === 'https:' ? 'wss' : 'ws'
    return `${proto}://${location.host}${path}`
  }

  async me(): Promise<MeView> {
    return this.api<MeView>('/v1/me')
  }

  /**
   * **世界的身份**（`GET /v1/home`，认证 Human）：`home_id` 是**世界级持久事实的归档键**
   * （事件 cursor / 系统消息 / 房间缓存按它分键，见 `api/worldIdentity.ts`）。
   *
   * 读不到就让它抛：调用方按"问不到 ⇒ 退回按凭据分键"处理（**不阻断连接**）。
   */
  async worldIdentity(): Promise<HomeView> {
    return this.api<HomeView>('/v1/home')
  }

  async listRooms(): Promise<RoomListEntry[]> {
    const envelope = await this.api<RoomListEnvelope>('/v1/rooms')
    return envelope.rooms.map((v) => toRoomListEntry(v))
  }

  async listMessages(roomId: string): Promise<{ messages: Message[]; nextBefore: string | null }> {
    const envelope = await this.api<MessageHistoryEnvelope>(
      `/v1/rooms/${roomId}/messages?limit=50`,
    )
    return { messages: envelope.messages.map(toMessage), nextBefore: envelope.next_before }
  }

  async loadOlder(roomId: string, before: string): Promise<{ messages: Message[]; nextBefore: string | null }> {
    const envelope = await this.api<MessageHistoryEnvelope>(
      `/v1/rooms/${roomId}/messages?limit=50&before=${encodeURIComponent(before)}`,
    )
    return { messages: envelope.messages.map(toMessage), nextBefore: envelope.next_before }
  }

  async sendMessage(
    roomId: string,
    input: {
      body?: string
      replyToId?: string
      attachments?: Attachment[]
      mentions?: string[]
      authorId: string
      /**
       * **这一次逻辑发送的 Idempotency-Key**。
       *
       * 给了就用它，没给才新生成一支 —— 这一格是**重试**要的：契约 §6.3 规定
       * 同 actor + 同 key + 同 payload 会重放**原** Message（200），所以重试必须
       * 复用首次那把 key，否则"重发"会变成"再发一条"（撞「绝不因重放重复公共消息」）。
       */
      idempotencyKey?: string
    },
  ): Promise<{ messageId: string; message?: Message; idempotencyReplayed?: boolean }> {
    const result = await this.api<SendMessageResponse>(`/v1/rooms/${roomId}/messages`, {
      method: 'POST',
      idempotencyKey: input.idempotencyKey ?? newIdempotencyKey(),
      body: {
        body: input.body?.trim() ? input.body : null,
        reply_to_message_id: input.replyToId ?? null,
        attachments: input.attachments?.map((a) => a.id) ?? [],
        // 显式提及（agent Participant ID；非 group 房间 Core 拒绝非空——
        // 前端只在 group Room 构造；空数组等价无提及）。
        mentions: input.mentions ?? [],
      },
    })
    return {
      messageId: result.message.id,
      message: toMessage(result.message),
      idempotencyReplayed: result.idempotency_replayed,
    }
  }

  async recallMessage(roomId: string, messageId: string): Promise<void> {
    await this.api<RecallResponse>(`/v1/rooms/${roomId}/messages/${messageId}`, { method: 'DELETE' })
  }

  async advanceReadCursor(roomId: string, messageId: string): Promise<void> {
    await this.api(`/v1/rooms/${roomId}/read-cursor`, {
      method: 'PUT',
      body: { message_id: messageId },
    })
  }

  /**
   * `PUT /v1/rooms/{room_id}/attention`：写**我自己**在这间 group 房的提醒档
   * （契约 `CORE_V1_IM_CONTRACT.md` §9.1，2026-10-19 窄解冻切片）。
   *
   * 请求体**只有 `level`**（服务端 `deny_unknown_fields`）：「写谁的」由认证上下文给出，
   * 请求体里没有主语。这是**状态写**（无幂等键、无版本号）：同值重写与改值都是同一个 200。
   *
   * 返回**服务端的回读值**（`{"attention": …}`）而不是入参：这一格的权威在世界上 ——
   * 万一服务端规范化了取值，界面应当跟着服务端，而不是跟着我们请求里写的那一份。
   * 回读值也走同一个宽容解析（认不出 ⇒ `all`，绝不抛在请求成功之后）。
   */
  async setRoomAttention(roomId: string, level: RoomAttention): Promise<RoomAttention> {
    const result = await this.api<PutAttentionResponse>(`/v1/rooms/${roomId}/attention`, {
      method: 'PUT',
      body: { level },
    })
    return parseRoomAttention(result.attention)
  }

  async uploadAttachment(roomId: string, file: File): Promise<{ attachmentId: string }> {
    const result = (await uploadMultipart(
      `/v1/rooms/${roomId}/attachments`,
      this.token,
      file,
      'file',
      this.serverUrl,
    )) as UploadAttachmentResponse
    return { attachmentId: result.attachment.id }
  }

  async fetchAttachmentBlob(roomId: string, attachmentId: string): Promise<Blob> {
    return fetchBlob(`/v1/rooms/${roomId}/attachments/${attachmentId}`, this.token, this.serverUrl)
  }

  async listContacts(): Promise<{ accepted: Participant[]; pending: Participant[] }> {
    const res = await this.api<ListContactsResponse>('/v1/contacts')
    return {
      accepted: res.contacts.filter((c) => c.status === 'accepted').map((c) => toParticipant(c.peer)),
      // **只列等我回答的那些**（`direction === 'incoming'`）：接受/拒绝的判据是
      // "我是不是 addressee"，把对方还在等的 outgoing 也列成"待处理"会让用户点了
      // 接受却拿到 404（批次三第三刀补 direction 之后，这里才有判据可用）。
      pending: res.contacts
        .filter((c) => c.status === 'pending' && c.direction === 'incoming')
        .map((c) => toParticipant(c.peer)),
    }
  }

  /**
   * 发起联系人请求。字段名 `participant_id`（批次三第二刀由 `human_id` 改名）：
   * 目标从此可以是任意 active 主体——Human 或 Agent 都可以加。
   *
   * 返回**回执**而不是 `void`：服务端在响应里已经给了对方是谁（`contact.peer` 自带 `kind`）
   * 与这条在等谁（`direction`）。丢掉它，界面就只剩「请求已发送」一句话 —— 而那句话会把
   * 「对方早就申请过我」和「我刚发出去」说成同一件事（`domain.ts` 的
   * `ContactRequestReceipt` 里列了全部四种处境）。
   */
  async sendContactRequest(participantId: string): Promise<ContactRequestReceipt> {
    const res = await this.api<CreateContactResponse>('/v1/contacts/requests', {
      method: 'POST',
      idempotencyKey: newIdempotencyKey(),
      body: { participant_id: participantId },
    })
    return {
      peer: toParticipant(res.contact.peer),
      status: res.contact.status,
      direction: res.contact.direction,
      replayed: res.idempotency_replayed,
    }
  }

  async acceptContact(humanId: string): Promise<void> {
    await this.api(`/v1/contacts/${humanId}/accept`, { method: 'POST' })
  }

  /**
   * 拒绝给我的那条 pending 申请（`POST /v1/contacts/{participant_id}/decline`，204）。
   *
   * 无 body、无幂等键：契约给它的语义是"一次决定"——段以 `declined` 闭合，重复调用落到
   * 已经闭合的那一段上，与"已经没有待答的申请"同形。
   */
  async declineContact(participantId: string): Promise<void> {
    await this.api(`/v1/contacts/${participantId}/decline`, { method: 'POST' })
  }

  /**
   * 单方面解除关系（`DELETE /v1/contacts/{participant_id}`，幂等 204）。
   *
   * ⚠ **不要在这里做"要不要确认"的判断**——那是页面的事。这一层只负责发出去并如实报错：
   * 服务端把"本来就没有 active 段"也答成 204，所以成功不代表"原来确实有一段"。
   */
  async removeContact(participantId: string): Promise<void> {
    await this.api(`/v1/contacts/${participantId}`, { method: 'DELETE' })
  }

  /**
   * 拉黑一个主体（`POST /v1/blocks/{participant_id}`，204）。
   *
   * 契约把它定义成 **拉黑 ⊇ 解除**：同一事务里先结束既有的 active 关系段，再写黑名单段。
   * 所以这一条**同时是一支解除**——页面上的文案必须说清，否则用户以为"只是屏蔽了消息"。
   * 无幂等键：成功与"已经拉黑过"都是 204（幂等由"每对每方向至多一个 active 段"承担）。
   */
  async blockParticipant(participantId: string): Promise<void> {
    await this.api(`/v1/blocks/${participantId}`, { method: 'POST' })
  }

  /** 解除拉黑（`DELETE /v1/blocks/{participant_id}`，幂等 204）。**不恢复关系**。 */
  async unblockParticipant(participantId: string): Promise<void> {
    await this.api(`/v1/blocks/${participantId}`, { method: 'DELETE' })
  }

  /** 我拉黑过的主体（`GET /v1/blocks`，只读）。只有"我拉黑的"这一半。 */
  async listBlocks(): Promise<BlockedPeer[]> {
    const res = await this.api<ListBlocksResponse>('/v1/blocks')
    return res.blocks.map((block) => ({
      ...toParticipant(block.peer),
      blockedAt: block.created_at,
    }))
  }

  async createHumanDirectRoom(humanId: string): Promise<{ roomId: string }> {
    const res = await this.api<CreateRoomResponse>('/v1/human-direct-rooms', {
      method: 'POST',
      idempotencyKey: newIdempotencyKey(),
      body: { human_id: humanId },
    })
    return { roomId: res.room.id }
  }

  /**
   * 与该 Agent 的那一间私人房。**一间房 = 一段关系**（Core 批次二）：同一对
   * Human–Agent 只有一间，已有则 Core 回放原房（200 `idempotency_replayed`），
   * 所以这里不需要（也没有）"要不要新建"的判断。`name` 不再是合法字段。
   */
  async createDirectRoom(agentId: string): Promise<{ roomId: string }> {
    const res = await this.api<CreateRoomResponse>('/v1/direct-rooms', {
      method: 'POST',
      idempotencyKey: newIdempotencyKey(),
      body: { agent_id: agentId },
    })
    return { roomId: res.room.id }
  }

  async createGroup(name: string): Promise<{ roomId: string }> {
    const res = await this.api<CreateRoomResponse>('/v1/groups', {
      method: 'POST',
      idempotencyKey: newIdempotencyKey(),
      body: { name },
    })
    return { roomId: res.room.id }
  }

  async createAgent(displayName: string): Promise<{ agentId: string }> {
    const res = await this.api<{ agent: { id: string } }>('/v1/agents', {
      method: 'POST',
      idempotencyKey: newIdempotencyKey(),
      body: { display_name: displayName },
    })
    return { agentId: res.agent.id }
  }

  async listOwnedAgents(): Promise<OwnedAgent[]> {
    const res = await this.api<{
      agents: Array<{ agent: { id: string; kind: string; display_name: string; status: string } }>
    }>('/v1/agents')
    // `status` 是**这个列表端点自己的事实**（契约：owner-scoped 管理视图，含 revoked 历史）
    // ⇒ 就地投影成 `revoked`，不下沉进共用的 `toParticipant`（理由见 `domain.ts` 那段）。
    return res.agents.map((a) => ({ ...toParticipant(a.agent), revoked: a.agent.status === 'revoked' }))
  }

  async getAgent(agentId: string): Promise<GetAgentResponse> {
    return this.api<GetAgentResponse>(`/v1/agents/${agentId}`)
  }

  async getAgentProfile(agentId: string): Promise<GetProfileResponse> {
    return this.api<GetProfileResponse>(agentProfilePath(agentId))
  }

  async putAgentProfile(
    agentId: string,
    input: {
      baseVersion: number
      shortDescription: string | null
      publicDescription: string | null
    },
  ): Promise<PutProfileResponse> {
    const request = buildAgentProfilePutRequest(agentId, input, newIdempotencyKey())
    return this.api<PutProfileResponse>(request.path, {
      method: request.method,
      idempotencyKey: request.idempotencyKey,
      body: request.body,
    })
  }

  /**
   * PUT /v1/agents/{agent_id}/profile/avatar：上传/替换头像（`CORE_V1_AVATAR_P0`）。
   *
   * Owner 唯一写路径；**单步 multipart**：`base_version` 文本 part ＋ `file` 文件 part。
   * 幂等键每次生成一支（与 `putAgentProfile` 同款）——契约要求它（`idempotency_key_required`
   * 是契约里列出的错误码）；版本与幂等裁决都在服务端事务内，客户端不缓存版本、不预判结果，写完重读元数据即可。
   */
  async putAgentAvatar(
    agentId: string,
    input: { baseVersion: number; file: File },
  ): Promise<PutProfileResponse> {
    const result = await uploadMultipart(
      `/v1/agents/${agentId}/profile/avatar`,
      this.token,
      input.file,
      'file',
      this.serverUrl,
      {
        method: 'PUT',
        fields: { base_version: String(input.baseVersion) },
        // **契约要求这个头**（缺了 im 直接回 `idempotency_key_required`）；
        // 每次生成一支即可。
        idempotencyKey: newIdempotencyKey(),
      },
    )
    return result as PutProfileResponse
  }

  /**
   * DELETE /v1/agents/{agent_id}/profile/avatar：摘除 ⇒ 推进一个新版本（avatar=null）。
   *
   * 与上传同一套版本/幂等裁决，只是没有字节：JSON body 带 `base_version`。
   * 历史版本行与其字节保留 —— 摘除只改最新投影。
   */
  async deleteAgentAvatar(
    agentId: string,
    input: { baseVersion: number },
  ): Promise<PutProfileResponse> {
    return this.api<PutProfileResponse>(`/v1/agents/${agentId}/profile/avatar`, {
      method: 'DELETE',
      idempotencyKey: newIdempotencyKey(),
      body: { base_version: input.baseVersion },
    })
  }

  /**
   * GET /v1/rooms/{room_id}/members/{participant_id}/avatar：取头像字节。
   *
   * **Room 成员路径是契约给的唯一读取面**（requester 与 target 都必须是该房间当前
   * active 成员）；无头像／非成员／跨房间一律 404 —— 调用方据此降级成 fallback，
   * 那是常态不是故障。所以"联系人页那种没有房间的地方"取不到头像**是设计**，
   * 不是缺口，不要在这里发明第二条读取路径。
   *
   * 实现走 `fetchBlob`（Bearer 头 + `res.blob()`）：**core 线没有客户端响应限额**，
   * 契约允许的 2 MiB 头像不会被限额截断（不是漏做）。
   */
  /**
   * GET /v1/me/profile：读**我自己**的名字与呈现（表达权第四刀，2026-10-17）。
   *
   * 名字来自身份事实、呈现来自版本链 —— 两件事并排，不是"合起来的对象"。
   * `profile === null` 表示**从未写过呈现**（不是错误）。
   */
  async getMyProfile(): Promise<GetMyProfileResponse> {
    return this.api<GetMyProfileResponse>('/v1/me/profile')
  }

  /**
   * PUT /v1/me/profile：改**我自己**的名字。
   *
   * **状态写**：`display_name` 是既有 Participant 身份事实（不在呈现版本链上）
   * ⇒ **不带 Idempotency-Key**、没有版本号 —— 同值重写天然幂等。
   * 校验在服务端（1..=80、禁 NUL、trim 非空，与兑换时同一套）。
   */
  async renameMe(displayName: string): Promise<{ display_name: string; request_id: string }> {
    return this.api<{ display_name: string; request_id: string }>('/v1/me/profile', {
      method: 'PUT',
      body: { display_name: displayName },
    })
  }

  /**
   * PUT /v1/me/profile/avatar：写**我自己**的头像（multipart 单步，版本链）。
   *
   * 与 Agent 那条**同一套**：`base_version` 文本 part ＋ `file` 文件 part ＋
   * 每次一支幂等键（契约要求）；版本与幂等裁决都在服务端事务内。
   */
  async putMyAvatar(input: { baseVersion: number; file: File }): Promise<PutProfileResponse> {
    const result = await uploadMultipart(
      '/v1/me/profile/avatar',
      this.token,
      input.file,
      'file',
      this.serverUrl,
      {
        method: 'PUT',
        fields: { base_version: String(input.baseVersion) },
        idempotencyKey: newIdempotencyKey(),
      },
    )
    return result as PutProfileResponse
  }

  /**
   * DELETE /v1/me/profile/avatar：摘除**我自己**的头像（新版本 avatar=null）。
   *
   * 历史版本行与其字节保留 —— 摘除只改最新投影。
   */
  async deleteMyAvatar(input: { baseVersion: number }): Promise<PutProfileResponse> {
    return this.api<PutProfileResponse>('/v1/me/profile/avatar', {
      method: 'DELETE',
      idempotencyKey: newIdempotencyKey(),
      body: { base_version: input.baseVersion },
    })
  }

  async fetchMemberAvatarBlob(roomId: string, participantId: string): Promise<Blob> {
    return fetchBlob(
      `/v1/rooms/${roomId}/members/${participantId}/avatar`,
      this.token,
      this.serverUrl,
      'avatar_download',
    )
  }

  async getRoom(roomId: string): Promise<GetRoomResponse> {
    return this.api<GetRoomResponse>(`/v1/rooms/${roomId}`)
  }

  async getMemberPresentations(roomId: string): Promise<GetMemberPresentationsResponse> {
    return this.api<GetMemberPresentationsResponse>(memberPresentationsPath(roomId))
  }

  async getRoomPresentation(roomId: string, participantId: string): Promise<GetPresentationResponse> {
    return this.api<GetPresentationResponse>(roomPresentationPath(roomId, participantId))
  }

  async putRoomPresentation(
    roomId: string,
    participantId: string,
    input: {
      baseVersion: number
      roomDisplayName: string | null
      roomRoleName: string | null
      roomRoleDescription: string | null
    },
  ): Promise<PutPresentationResponse> {
    const request = buildRoomPresentationPutRequest(roomId, participantId, input, newIdempotencyKey())
    return this.api<PutPresentationResponse>(request.path, {
      method: request.method,
      idempotencyKey: request.idempotencyKey,
      body: request.body,
    })
  }

  async createBackendBinding(agentId: string, label: string): Promise<void> {
    await this.api<CreateBindingResponse>(`/v1/agents/${agentId}/backend-bindings`, {
      method: 'POST',
      idempotencyKey: newIdempotencyKey(),
      body: { label },
    })
  }

  async listBackendBindings(agentId: string): Promise<BackendBindingView[]> {
    const envelope = await this.api<ListBindingsResponse>(`/v1/agents/${agentId}/backend-bindings`)
    return envelope.bindings
  }

  async revokeBackendBinding(agentId: string, bindingId: string): Promise<void> {
    await this.api(`/v1/agents/${agentId}/backend-bindings/${bindingId}`, { method: 'DELETE' })
  }

  async issueCredential(
    agentId: string,
    bindingId: string,
    replacesCredentialId: string | null,
  ): Promise<{ token: string | null }> {
    const res = await this.api<IssueCredentialResponse>(
      `/v1/agents/${agentId}/backend-bindings/${bindingId}/credentials`,
      {
        method: 'POST',
        idempotencyKey: newIdempotencyKey(),
        body: { replaces_credential_id: replacesCredentialId },
      },
    )
    return { token: res.token }
  }

  async listCredentials(agentId: string, bindingId: string): Promise<CredentialView[]> {
    const envelope = await this.api<ListCredentialsResponse>(
      `/v1/agents/${agentId}/backend-bindings/${bindingId}/credentials`,
    )
    return envelope.credentials
  }

  async revokeCredential(agentId: string, bindingId: string, credentialId: string): Promise<void> {
    await this.api(`/v1/agents/${agentId}/backend-bindings/${bindingId}/credentials/${credentialId}`, {
      method: 'DELETE',
    })
  }

  async revokeAgent(agentId: string): Promise<void> {
    await this.api(`/v1/agents/${agentId}`, { method: 'DELETE' })
  }

  async listRoomMembers(roomId: string): Promise<RoomMembersItem[]> {
    const envelope = await this.api<RoomMembersEnvelope>(`/v1/rooms/${roomId}/members`)
    return envelope.members
  }

  /**
   * 主体进行态快照（`GET /v1/rooms/{id}/live`）：谁在线、谁正在写。
   *
   * 纯读、易失：读不到就如实说读不到，**不缓存成"它在线"**。可见性与成员
   * 名单同一条（viewer 必须是该房 active 成员，否则 404）。
   */
  async roomLive(roomId: string): Promise<RoomLiveSnapshot> {
    return this.api<RoomLiveSnapshot>(`/v1/rooms/${roomId}/live`)
  }

  /**
   * 声明「我正在这间房写」（`POST /v1/rooms/{id}/typing`，204）。
   *
   * **best-effort，绝不抛**：进行态是易失信号，丢一条不影响任何消息、任何
   * 事实——所以调用方不需要 try/catch，也不该因为它失败去打扰用户。
   */
  async notifyTyping(roomId: string): Promise<void> {
    try {
      await this.api(`/v1/rooms/${roomId}/typing`, { method: 'POST' })
    } catch {
      // 刻意吞掉：见上。服务端按 TTL 过期，客户端下次输入会再报一次。
    }
  }

  /**
   * 签发一张**邀请码**（`POST /v1/home/pairing-grants`，`grants: new_human`）。
   *
   * 一次性：返回的 `token` 只在这次响应里出现，服务端只留摘要——界面必须
   * 当场显示给人复制，之后谁也拿不回来（这与"家钥匙"同一条纪律）。
   */
  async issueInvite(): Promise<PairingGrantIssue> {
    return this.api<PairingGrantIssue>('/v1/home/pairing-grants', {
      method: 'POST',
      body: { grants: 'new_human' },
    })
  }

  /** 列出我签发的未消费凭据（邀请与设备配对同表，按 `grants` 区分用途）。 */
  async listGrants(): Promise<PairingGrantListEnvelope> {
    return this.api<PairingGrantListEnvelope>('/v1/home/pairing-grants')
  }

  /** 撤销一张凭据（幂等：已撤销的重复撤销仍然成功）。 */
  async revokeGrant(grantId: string): Promise<void> {
    await this.api(`/v1/home/pairing-grants/${grantId}`, { method: 'DELETE' })
  }

  async inviteMember(roomId: string, participantId: string): Promise<void> {
    await this.api<InviteMemberResponse>(`/v1/rooms/${roomId}/members`, {
      method: 'POST',
      idempotencyKey: newIdempotencyKey(),
      body: { participant_id: participantId },
    })
  }

  async removeMember(roomId: string, participantId: string): Promise<void> {
    await this.api(`/v1/rooms/${roomId}/members/${participantId}`, { method: 'DELETE' })
  }

  async leaveRoom(roomId: string): Promise<void> {
    await this.api(`/v1/rooms/${roomId}/leave`, { method: 'POST' })
  }

  async transferOwnership(roomId: string, participantId: string): Promise<void> {
    await this.api<TransferOwnerResponse>(`/v1/rooms/${roomId}/owner-transfer`, {
      method: 'POST',
      idempotencyKey: newIdempotencyKey(),
      body: { participant_id: participantId },
    })
  }

  async revokeSession(): Promise<void> {
    await this.api(`/v1/sessions/current`, { method: 'DELETE' })
  }

  async markRoomRead(roomId: string): Promise<void> {
    const latest = await this.api<MessageHistoryEnvelope>(
      `/v1/rooms/${roomId}/messages?limit=1`,
    )
    const last = latest.messages[latest.messages.length - 1]
    if (last) {
      await this.advanceReadCursor(roomId, last.id)
    }
  }

  async listEvents(after?: string): Promise<{ events: AppEvent[]; nextAfter: string }> {
    const query = after ? `?after=${encodeURIComponent(after)}` : ''
    const envelope = await this.api<HumanEventsEnvelope>(`/v1/human/events${query}`)
    const events: AppEvent[] = envelope.events.map((e) => this.projectEvent(e))
    return { events, nextAfter: envelope.next_after }
  }

  private projectEvent(e: HumanEventEnvelope): AppEvent {
    const base = {
      eventId: e.id,
      kind: e.kind,
      happenedAt: e.created_at,
      rawSnapshot: e.snapshot,
    }
    const s = e.snapshot
    if (e.kind === 'message.created') {
      return {
        ...base,
        message: toMessage(s as unknown as MessageView),
        roomId: e.room_id,
      }
    }
    if (e.kind === 'message.recalled') {
      return {
        ...base,
        roomId: e.room_id,
        message: {
          id: (s as { message_id?: string })?.message_id ?? '',
          roomId: e.room_id,
          authorId: '',
          body: '',
          sentAt: '',
          delivery: 'delivered',
          recalledAt: e.created_at,
        } as Message,
      }
    }
    return { ...base, roomId: e.room_id }
  }

  subscribeWakeup(handler: () => void): () => void {
    this.wakeupHandler = handler
    this.openWs()
    return () => {
      this.wakeupHandler = undefined
      this.ws?.close()
      this.ws = null
      if (this.reconnectTimer) {
        clearTimeout(this.reconnectTimer)
        this.reconnectTimer = null
      }
    }
  }

  private openWs() {
    if (this.ws) return
    const ws = new WebSocket(this.wsUrl('/v1/human/events/stream'))
    this.ws = ws
    ws.onopen = () => {
      this.reconnectAttempt = 0
      this.onConnectionChange?.('online')
      ws.send(JSON.stringify({ type: 'authenticate', token: this.token }))
    }
    ws.onmessage = (ev) => {
      try {
        const frame = JSON.parse(String(ev.data))
        if (frame.type === 'authenticated' || frame.type === 'events.available') {
          this.wakeupHandler?.()
        }
      } catch {
        // ignore malformed frames
      }
    }
    ws.onclose = (ev) => {
      this.ws = null
      // 认证失败等致命关闭码：停止自动重连，交给用户显式重连
      if (ev.code === 1008 || ev.code === 4401 || ev.code === 4403) {
        this.onConnectionChange?.('offline')
        return
      }
      this.onConnectionChange?.('offline')
      const delay = Math.min(30_000, 2_000 * 2 ** this.reconnectAttempt)
      this.reconnectAttempt += 1
      this.reconnectTimer = setTimeout(() => this.openWs(), delay)
    }
  }

  // ── 通话（`CORE_V1_CALL.md`，Call P0 窄解冻切片）──────────────────────────
  //
  // 这一面的六个方法与**第二条 WS** 都在这儿：`/v1/call/stream` 与 Human Event WS
  // **是两条独立流**（契约 §4.1：独立 notifier、不污染 `events.available` 语义）。
  // ⚠ 媒体的一个字都不在这里：Core 只中继 SDP/ICE，媒体是两端之间的 WebRTC P2P。

  async initiateCall(
    roomId: string,
    calleeId: string,
    media?: 'audio' | 'video',
  ): Promise<CallInitiateResponse> {
    const body: CallInitiateRequest = { room_id: roomId, callee_id: calleeId }
    // `media` 缺省 audio 是**服务端的**默认；这里只在显式给了非 audio 时才带上，
    // 免得把"我们没意见"写成"我们指定了 audio"（服务端 `deny_unknown_fields` 也要求字段形状一致）。
    if (media && media !== 'audio') body.media = media
    // ⚠ `body` 传**对象**：`fetchJson` 自己 `JSON.stringify`（见 `client.ts`）。
    //   2026-10-09 真机探针抓到的缺陷：这里曾传 `JSON.stringify(body)` ⇒ **二次编码** ⇒
    //   服务端读到的是 JSON 字符串而不是对象 ⇒ 400 `invalid_request`，界面上只有一句
    //   「请求不合法」。**单测与边界守卫都看不见它**（守卫只对路径与方法）——
    //   现在由 `tests/call-api.test.ts` 钉住"发出去的请求体是一个 JSON 对象"。
    return this.api<CallInitiateResponse>('/v1/calls', { method: 'POST', body })
  }

  async getCall(callId: string): Promise<CallEnvelope> {
    return this.api<CallEnvelope>(`/v1/calls/${encodeURIComponent(callId)}`)
  }

  async acceptCall(callId: string): Promise<CallEnvelope> {
    return this.api<CallEnvelope>(`/v1/calls/${encodeURIComponent(callId)}/accept`, { method: 'POST' })
  }

  async declineCall(callId: string): Promise<CallEnvelope> {
    return this.api<CallEnvelope>(`/v1/calls/${encodeURIComponent(callId)}/decline`, { method: 'POST' })
  }

  async hangupCall(callId: string): Promise<CallEnvelope> {
    return this.api<CallEnvelope>(`/v1/calls/${encodeURIComponent(callId)}/hangup`, { method: 'POST' })
  }

  /** 204 无响应体 ⇒ 返回 `void`（**不假装拿到了 Call**：这一格本来就没有回执）。 */
  async signalCall(callId: string, request: CallSignalRequest): Promise<void> {
    // ⚠ 同 `initiateCall`：`body` 是**对象**（`fetchJson` 自己序列化）。传字符串就是二次编码 ⇒ 400。
    await this.api<void>(`/v1/calls/${encodeURIComponent(callId)}/signal`, {
      method: 'POST',
      body: request,
    })
  }

  /**
   * 订阅 Call-stream（第二条 WS）。返回退订函数。
   *
   * 与唤醒流同一套形状（首帧 `authenticate`、致命关闭码不重连、指数退避），差别只有两处：
   * ① 这一条**带事实**（帧要交给调用方）；② **不重放** ⇒ 重连成功后再叫一次 `handler`
   * 让调用方用 `getCall` 重同步（契约 §4.1 最后一条）。
   */
  subscribeCallStream(handler: (frame: CallStreamEvent) => void, onResync?: () => void): () => void {
    this.callStreamHandler = handler
    this.callStreamResync = onResync
    this.openCallStream()
    return () => {
      this.callStreamHandler = undefined
      this.callStreamResync = undefined
      this.callStream?.close()
      this.callStream = null
      if (this.callStreamTimer) {
        clearTimeout(this.callStreamTimer)
        this.callStreamTimer = null
      }
    }
  }

  private openCallStream() {
    if (this.callStream) return
    const ws = new WebSocket(this.wsUrl('/v1/call/stream'))
    this.callStream = ws
    ws.onopen = () => {
      this.callStreamAttempt = 0
      ws.send(JSON.stringify({ type: 'authenticate', token: this.token }))
      // ⚠ **每次接上都重同步一次**（契约 §4.1：这一条流不重放）——重连期间错过的
      // `call.state`（对方挂了、超时结束了）不重同步就永远补不回来，界面会一直停在通话中。
      this.callStreamResync?.()
    }
    ws.onmessage = (ev) => {
      const frame = parseCallStreamFrame(String(ev.data))
      // ⚠ **认不出的帧一律丢掉**（fail closed）：这一格不许把"我不认识"当成某种已知事件。
      if (frame) this.callStreamHandler?.(frame)
    }
    ws.onclose = (ev) => {
      this.callStream = null
      if (ev.code === 1008 || ev.code === 4401 || ev.code === 4403) return
      const delay = Math.min(30_000, 2_000 * 2 ** this.callStreamAttempt)
      this.callStreamAttempt += 1
      this.callStreamTimer = setTimeout(() => this.openCallStream(), delay)
    }
  }
}
