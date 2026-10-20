import type {
  Attachment,
  BlockedPeer,
  ContactRequestReceipt,
  Message,
  OwnedAgent,
  Participant,
  Room,
} from '@/api/domain'
import type {
  BackendBindingView,
  CredentialView,
  GetAgentResponse,
  GetMemberPresentationsResponse,
  GetPresentationResponse,
  GetProfileResponse,
  GetRoomResponse,
  HomeView,
  MeView,
  PutProfileResponse,
  PutPresentationResponse,
  RoomLiveSnapshot,
  RoomMembersItem,
  GetMyProfileResponse,
} from './contracts'

export interface RoomListEntry {
  room: Room
  peer: Participant[]
}

/**
 * **世界侧传输层的事实域**（2026-09-25 拆分刀；2026-09-28 服务层退役后剩三个）。
 *
 * ## 为什么拆
 *
 * 原先只有一个 `ChatApi`，62 个方法。它是**单一平面的**接口——家平面（`homes.ts` /
 * `homeStatus.ts` / `homeConfig.ts`）**从来没进过它**：家那三样走自己的 `fetch`＋家钥匙，
 * 不经 `getApi()`、不随连接状态切换。所以这里要治的不是"两个平面粘在一起"，
 * 而是**世界侧这一个平面内部按事实域混装**：房间与消息、关系、参与者管理、
 * 服务与凭据几件事挤在一个类型里，"哪个方法属于哪件事"只能靠注释回答。
 *
 * ## 拆的依据：事实域，不是调用顺序
 *
 * | 接口 | 那件事 | 事实所有者 |
 * |---|---|---|
 * | {@link RoomApi} | 房间、成员、消息、附件、read cursor、群内呈现、Live | im 服务端 |
 * | {@link RelationApi} | 联系人、群、建房 | im 服务端 |
 * | {@link ParticipantApi} | Agent 所有权、公共资料、头像、后端绑定、凭据、撤销 | im 服务端 |
 *
 * 三者**都是世界侧事实**，都由 `CoreApi` 实现。拆开是为了让"这件事归谁"从注释
 * 变成类型，不是为了隔离平面。
 *
 * ⚠ **2026-09-28**：原表还有第四个域 `ServiceApi`（Publication / Service Access /
 * 服务直连房）——它随 **Agent 服务层整体退役**删除（服务声明层是工作态，不是 IM
 * 事实，违反「执行过程不经过 IM」那条红线）。**"服务与凭据"这一栏现在只剩凭据。**
 *
 * ## 删掉了什么（同刀）
 *
 * **8 个零实现的可选方法**：`retryMessage` / `togglePinned` / `toggleMuted` /
 * `deleteRoom` / `getUnreadMessageIds` / `getAgentActivity` / `getDeliveryOverride` /
 * `isMessageRecalled`。它们当年只为 `api/mock.ts` 存在，**而那个文件已经不在工作树里**，
 * `CoreApi` 一个都没实现（拆分前 62 个声明 − 8 = 54 个实现，逐个数对得上）
 * ⇒ store 里那几处 `if (api?.xxx)` 是永远进不去的死分支，UI 上还挂着拨了没反应的开关
 * （`RoomInfoDrawer` 的置顶/免打扰、`MessageBubble` 的「点击重试」）。删掉它们让这些
 * **静默失效**变成编译错误，而不是继续假装存在。
 *
 * ## 删完之后：能力去哪了（**别把"删了方法"读成"删了功能"**）
 *
 * 那 8 个名字里有三个对应**真实的产品能力**，它们的去处各不相同，各有各的权威：
 *
 * | 能力 | 它归谁 | 现在住哪 |
 * |---|---|---|
 * | 置顶 / 免打扰 | **用户侧偏好**（我的看法，不是公共事实） | `api/roomPrefs.ts`（经 `api/storage.ts` 窄口落本机），动作在 store 的 `togglePinned` / `toggleMuted` |
 * | 消息重试 | **发送状态流**（契约 §6.3 的幂等重放） | store 的 `retry`：复用 pending 上那把 Idempotency-Key，`sendMessage` 现在接受并透传它 |
 * | 删除会话 | **没有去处** —— 房间是公共事实，Core v1 无删除端点，架构红线明写「不引入房间删除/状态/改名」 | 菜单项已移除；退出房间走 Core 真有的 `leave` |
 *
 * ⚠ 这条表是这一刀最容易读错的地方：**"问 Core 要"与"不该问 Core"是两件事**。
 * 前两个曾经被挂在零实现的方法上，看起来像"Core 还没做"，其实是**问错了地方**；
 * 只有第三个是真的没有能力。
 *
 * ⚠ **数过的账**：本刀只删这 8 个。`getStreamingAll` 这个名字**从来不是声明**——
 * 它只在 `store.ts` 的两行注释里被当作"已删除的 mock 路径"提及（那两行是历史注记，
 * 不在本刀范围内）。所以别把它算进这一刀，也别以为还有别的"残留方法"没清。
 *
 * ## optional 为什么保留
 *
 * 每个方法的 `?` 与拆分前**逐字一致**：`uiPolicy.ts` 的能力探测（`supportsAgentRevoke`
 * 等）就是按"传输层真的实现了那个方法"判的，把 optional 改成 required 会同时废掉
 * 那套探测与它的测试。本刀只画边界、不改能力语义。
 */

// ── 一、房间与消息（im 服务端）──────────────────────────────────────────────

export interface RoomApi {
  readonly mode: 'core'
  onUpdate?: () => void
  /**
   * 连接状态回调（`online` / `offline`）：**由 store 接线**，用来驱动事件补拉与重连。
   *
   * 它原先只在 `CoreApi` 上声明（接口里没有），而 store 拿的是具体类所以编译得过——
   * 那是"接口没有完整描述它自己的实现"。补在这里：它属于**世界侧连接生命周期**这一件事，
   * 与 `onUpdate` 同一档。
   */
  onConnectionChange?: (state: 'online' | 'offline') => void

  me(): Promise<MeView>
  /**
   * **世界的身份**（`GET /v1/home`，认证 Human）：回答"我连的是哪个世界"。
   *
   * 可选：演示通道（mock）与还没实现这一面的实现都没有它 ⇒ 调用方退回**按凭据分键**
   * （`api/worldIdentity.ts` 的 `worldKeyOf`），行为与从前一字不差。
   */
  worldIdentity?(): Promise<HomeView>
  listRooms(): Promise<RoomListEntry[]>
  listMessages(roomId: string): Promise<{ messages: Message[]; nextBefore: string | null }>
  /** 向后翻页（before 是 opaque cursor，绝不解析）。core 实现；mock 无。 */
  loadOlder?(
    roomId: string,
    before: string,
  ): Promise<{ messages: Message[]; nextBefore: string | null }>
  sendMessage(
    roomId: string,
    input: {
      body?: string
      replyToId?: string
      attachments?: Attachment[]
      /** 显式提及（agent Participant ID 列表）：结构化唤醒意图，仅 group Room
       * 允许非空（契约 CORE_V1_HUMAN_ROOMS_AND_MENTIONS.md §4.4）。 */
      mentions?: string[]
      authorId: string
      /** 每个逻辑发送一个随机可见 ASCII key；网络重试复用同一 key。 */
      idempotencyKey?: string
    },
  ): Promise<{ messageId: string; message?: Message; idempotencyReplayed?: boolean }>
  recallMessage(roomId: string, messageId: string): Promise<void>
  advanceReadCursor(roomId: string, messageId: string): Promise<void>
  markRoomRead?(roomId: string): Promise<void>
  /** Upload returns server public metadata; attachmentId remains for Core compatibility. */
  uploadAttachment?(roomId: string, file: File): Promise<{ attachmentId: string; attachment?: Attachment }>
  fetchAttachmentBlob?(roomId: string, attachmentId: string): Promise<Blob>
  /**
   * 主体进行态快照（`GET /v1/rooms/{id}/live`）：这间房里谁在线、谁正在写。
   * 易失投影，随读随取，不参与任何耐久事实。
   */
  roomLive?(roomId: string): Promise<RoomLiveSnapshot>
  /**
   * 声明「我正在这间房写」（`POST /v1/rooms/{id}/typing`，204，无参数）。
   * 易失、best-effort：失败**不报错**（进行态丢了不影响任何消息）。
   */
  notifyTyping?(roomId: string): Promise<void>
  getRoom?(roomId: string): Promise<GetRoomResponse>
  listRoomMembers?(roomId: string): Promise<RoomMembersItem[]>
  inviteMember?(
    roomId: string,
    participantId: string,
    /** core 模式忽略（core.ts 内部自生成）。 */
    idempotencyKey?: string,
  ): Promise<void | boolean>
  removeMember?(roomId: string, participantId: string): Promise<void>
  leaveRoom?(roomId: string): Promise<void>
  transferOwnership?(
    roomId: string,
    participantId: string,
    /** core 模式忽略（core.ts 内部自生成）。 */
    idempotencyKey?: string,
  ): Promise<void | boolean>
  // Group Room Presentation（CORE_V1_PARTICIPANT_PRESENTATION.md §2.2/§7）。
  // 摘要读取受当前 active Room Membership 可见性裁决；详情写只接受当前 group
  // room 的 active owner，目标必须是 active Agent 成员。PUT 由 core.ts 内部
  // 生成 Idempotency-Key（调用方传入的 `idempotencyKey` 被忽略）；新 key 时
  // base_version 必须等于当前最新版本。
  getMemberPresentations?(
    roomId: string,
  ): Promise<GetMemberPresentationsResponse>
  getRoomPresentation?(
    roomId: string,
    participantId: string,
  ): Promise<GetPresentationResponse>
  putRoomPresentation?(
    roomId: string,
    participantId: string,
    input: {
      baseVersion: number
      roomDisplayName: string | null
      roomRoleName: string | null
      roomRoleDescription: string | null
      idempotencyKey?: string
    },
  ): Promise<PutPresentationResponse>
  revokeSession?(): Promise<void>
  listEvents(after?: string): Promise<{ events: AppEvent[]; nextAfter: string }>
  subscribeWakeup(handler: () => void): () => void
}

// ── 二、关系与房间创建（im 服务端）──────────────────────────────────────────

export interface RelationApi {
  listContacts?(): Promise<{
    accepted: Participant[]
    pending: Participant[]
  }>
  /**
   * 向**任意 active 主体**发起关系申请（Human 或 Agent，同一串 `participant_id`）。
   * `idempotencyKey` 由 core.ts 内部自生成，调用方传入的值被忽略（每个「逻辑发送」一支新 key）。
   * 授权/关系裁决全在 Core。
   *
   * ⚠ 回执**必须**用上：它带着「对方是谁」（含 `kind`）与「这条在等谁」（`direction`）。
   * 只报一句"已发送"，会把「对方早就申请过我」说成「我刚发出去」。
   */
  sendContactRequest?(
    participantId: string,
    idempotencyKey?: string,
  ): Promise<ContactRequestReceipt>
  /** 作为 addressee 接受 pending 联系人请求。无 body、无 Idempotency-Key。 */
  acceptContact?(humanId: string): Promise<void>
  /**
   * **拒绝**给我的那条 pending 申请（段以 `declined` 闭合）。
   *
   * 判据只有"我是不是 addressee"——所以界面上只能对 `direction === 'incoming'` 的那条画它。
   * 无 body、无 Idempotency-Key；成功 204。
   */
  declineContact?(participantId: string): Promise<void>
  /**
   * **单方面解除**关系（段以 `ended` 闭合 + 关闭双方成员周期）。
   *
   * 任一方即时生效、不需对方同意、**不通知对端**；幂等 204（本来就没有 active 段也是 204）。
   * ⚠ 它的后果不止"列表里少一个人"：**直接房永久封存**（双方不能发也不能读，历史留在服务端，
   * 房间从双方列表消失），复合是**另建一间新房**。调用方必须先把这件事告诉用户。
   */
  removeContact?(participantId: string): Promise<void>
  /**
   * **拉黑**一个主体。契约把它定义为**拉黑 ⊇ 解除**：同一事务里先结束既有的 active 段。
   *
   * 被拉黑方**一无所知**（不泄露）、**不阻断**双方在群里的发言；成功与"已经拉黑过"都是 204。
   */
  blockParticipant?(participantId: string): Promise<void>
  /**
   * **解除拉黑**（幂等 204）。
   *
   * ⚠ **不恢复关系**——契约明写不做关系恢复；解除之后你们仍然是陌生人。
   */
  unblockParticipant?(participantId: string): Promise<void>
  /**
   * 我拉黑过的主体（`GET /v1/blocks`，只读）。
   *
   * ⚠ 只有**我拉黑的**这一半：不存在"谁拉黑了我"的读取面（拉黑不泄露）。
   */
  listBlocks?(): Promise<BlockedPeer[]>
  createHumanDirectRoom?(humanId: string): Promise<{ roomId: string }>
  /**
   * 与该 Agent 的那一间私人房：**一间房 = 一段关系**（Core 批次二）——已有就由
   * Core 回放原房（响应 `idempotency_replayed=true`），不会新建第二间。请求体只有
   * `agent_id`；`name` 已是未知字段（direct 房没有标题）。
   */
  createDirectRoom?(agentId: string): Promise<{ roomId: string }>
  createGroup?(
    name: string,
    /** core 模式忽略（core.ts 内部自生成）。 */
    idempotencyKey?: string,
  ): Promise<{ roomId: string }>
}

// ── 三、参与者管理：Agent 所有权与接入（im 服务端）──────────────────────────

export interface ParticipantApi {
  createAgent?(
    displayName: string,
    /** core 模式忽略（core.ts 内部自生成）。 */
    idempotencyKey?: string,
  ): Promise<{ agentId: string }>
  listOwnedAgents?(): Promise<OwnedAgent[]>
  getAgent?(agentId: string): Promise<GetAgentResponse>
  getAgentProfile?(agentId: string): Promise<GetProfileResponse>
  putAgentProfile?(
    agentId: string,
    input: {
      baseVersion: number
      shortDescription: string | null
      publicDescription: string | null
      /**
       * core 模式忽略（core.ts 仍按既有行为每次调用生成新 key，不回退）。
       * key 绝不入持久层/DOM/日志。
       */
      idempotencyKey?: string
    },
  ): Promise<PutProfileResponse>
  /**
   * 头像（`CORE_V1_AVATAR_P0.md`）：core 直连走 Bearer + `newIdempotencyKey()`
   * （契约要求 key；本线不做"按键复用"那套纪律，写完重读元数据即可）。
   *
   * `idempotencyKey` 因此是**可选**的：core 线不读它（自己生成）。
   */
  putAgentAvatar?(
    agentId: string,
    input: { baseVersion: number; file: File; idempotencyKey?: string },
  ): Promise<PutProfileResponse>
  deleteAgentAvatar?(
    agentId: string,
    input: { baseVersion: number; idempotencyKey?: string },
  ): Promise<PutProfileResponse>
  /**
   * 取某个成员在**这一间房**里的头像字节（`CORE_V1_AVATAR_P0` §3.4）。
   *
   * core 的认证是 Bearer 头，`<img>` 带不了 ⇒ 必须 fetch 回来做 objectURL。
   */
  fetchMemberAvatarBlob?(roomId: string, participantId: string): Promise<Blob>
  /**
   * **人自己的资料**（表达权第四刀，2026-10-17 主人批准）：读自己的名字与呈现、
   * 改自己的名字、写/摘自己的头像。
   *
   * ⚠ 与 Agent 那两条的**区别只有一处**：改名是**状态写**（`display_name` 是既有
   * Participant 身份事实，不在呈现版本链上）⇒ 它**不带** `Idempotency-Key`
   * （同值重写天然幂等）；头像照旧走版本链那一套（key + `base_version`）。
   */
  getMyProfile?(): Promise<GetMyProfileResponse>
  renameMe?(displayName: string): Promise<{ display_name: string; request_id: string }>
  putMyAvatar?(input: { baseVersion: number; file: File }): Promise<PutProfileResponse>
  deleteMyAvatar?(input: { baseVersion: number }): Promise<PutProfileResponse>
  createBackendBinding?(agentId: string, label: string): Promise<void>
  listBackendBindings?(agentId: string): Promise<BackendBindingView[]>
  revokeBackendBinding?(agentId: string, bindingId: string): Promise<void>
  issueCredential?(agentId: string, bindingId: string, replacesCredentialId: string | null): Promise<{ token: string | null }>
  listCredentials?(agentId: string, bindingId: string): Promise<CredentialView[]>
  revokeCredential?(agentId: string, bindingId: string, credentialId: string): Promise<void>
  revokeAgent?(agentId: string): Promise<void>
}

/**
 * **世界侧传输层的完整形状**：三个事实域的合取。
 *
 * 调用方（`store.ts` 的 `api` 变量、`getApi()`）用这个合取，因为它要按事实域
 * 分别取数；而"某一段代码需要哪几件事"应当用上面**单个**接口来标注 —— 那是这次
 * 拆分真正要买到的东西。
 *
 * ⚠ 家平面**不在**这里（`homes.ts` / `homeStatus.ts` / `homeConfig.ts` 有自己的
 * 传输路径与凭据），所以本类型不叫 `ChatApi` 也不再暗示"所有东西都从这一个口进"。
 *
 * ⚠ **2026-09-28**：原为四个事实域 —— 第四个是 `ServiceApi`（Publication /
 * Service Access / 服务直连房），随 **Agent 服务层整体退役**删除。三域都在，
 * 只是少了一件本来就不该由 IM 承载的事。
 */
/**
 * 家的准入凭据（世界侧事实域）：**邀请一个人进这个世界**。
 *
 * 只做三件事——签发一张邀请码（一次性、72 小时、可撤销）、列出自己签发的、
 * 撤销一张。**不做匿名自注册**：一个世界的入口只有"建一个世界"与"被请进
 * 一个世界"两条（`PRODUCT_VISION.md` 当前阶段）。
 */
export interface HomeApi {
  /** 签发一张邀请码（`grants: 'new_human'`）。返回的 token **只出现这一次**。 */
  issueInvite(): Promise<import('./contracts').PairingGrantIssue>
  /** 列出**我**签发的、还没被用掉的凭据（设备配对与邀请都在里面）。 */
  listGrants(): Promise<import('./contracts').PairingGrantListEnvelope>
  /** 撤销一张（幂等）。 */
  revokeGrant(grantId: string): Promise<void>
}

export type WorldApi = RoomApi & RelationApi & ParticipantApi & HomeApi

// ── 五、通话（im 服务端 `CORE_V1_CALL.md`，Call P0 窄解冻切片）────────────────

/**
 * **通话面**：状态机与信令（`POST /v1/calls…` ＋ `GET /v1/call/stream`）。
 *
 * ⚠ **媒体不在这一层**：媒体是**两端客户端之间的 WebRTC P2P**，Core 永不接收/存储/转发
 * 媒体字节（契约 §1/§6）。这一层只做"谁在呼谁、现在什么状态、把 SDP/ICE 原样转给对端"。
 *
 * 为什么**不并进 `WorldApi`**（而 `RoomApi` 那四域都在里面）：演示通道（`mock.ts`）**没有通话**
 * —— 它不是"世界里的事实少了半边"，而是这一格今天只有真实世界有。并进去就得给 mock 编一套
 * 假的通话状态（本仓明令：`fixtures` 只服务演示通道，且绝不假装支持真实能力）。
 * ⇒ 判据只有一处：`hasCallApi(api)`（`lib/callSession.ts`），页面不许自己 `as any` 或
 * 按"有没有壳"之类的旁证去猜。
 */
export interface CallApi {
  /**
   * 发起（`POST /v1/calls`）：**201 = 新建 / 200 = 复用既有 active Call**（读 `created`）。
   *
   * 契约边界：至少一方在**调用时刻**是该 Room 的 active 成员、caller ≠ callee、双方 active；
   * 任一方已在别的 active Call ⇒ 409 `call_party_busy`。
   */
  initiateCall(
    roomId: string,
    calleeId: string,
    media?: 'audio' | 'video',
  ): Promise<import('./contracts').CallInitiateResponse>
  /** 读当前状态（`GET /v1/calls/{id}`）：**重连/错过唤醒后的唯一重同步手段**（流不重放）。 */
  getCall(callId: string): Promise<import('./contracts').CallEnvelope>
  /** 接听（**仅 callee**，`ringing → in_progress`）；非 callee ⇒ 404、非 ringing ⇒ 409。 */
  acceptCall(callId: string): Promise<import('./contracts').CallEnvelope>
  /** 拒接（**仅 callee**，`ringing → ended(decline_callee)`）。 */
  declineCall(callId: string): Promise<import('./contracts').CallEnvelope>
  /** 挂断（任一方；`ended(hangup_caller|hangup_callee)` 按动作方）。 */
  hangupCall(callId: string): Promise<import('./contracts').CallEnvelope>
  /**
   * 中继一条信令（`POST /v1/calls/{id}/signal`，**204 无响应体**）。
   *
   * 方向由 Core 强制：offer 只可 caller→callee、answer 只可 callee→caller、ice 双向；
   * 方向/角色错误与 ended 之后一律 409 `call_state_conflict`。对端没有流 ⇒ **静默丢弃**
   * （fail closed，媒体面由 RTC 重协商兜底）——所以"发出去了"不等于"对方收到了"。
   */
  signalCall(callId: string, request: import('./contracts').CallSignalRequest): Promise<void>
  /**
   * **Call-stream**（`GET /v1/call/stream`）：与 Human Event WS **是两条独立流**
   * （不污染 `events.available` 语义）。返回退订函数。
   *
   * 契约边界：只投**当前在线**的连接、**不重放** ⇒ 断线期间错过的信令不补发。
   * 所以有第二个回调 `onResync`：**流每次接上（含重连）时叫一次**，调用方用 `getCall`
   * 重同步当前状态（媒体由 RTC 重协商兜底）—— 不重同步就会出现"对方早挂了、界面还在通话中"。
   */
  subscribeCallStream(
    handler: (frame: import('./contracts').CallStreamEvent) => void,
    onResync?: () => void,
  ): () => void
}


export interface AppEvent {
  eventId: string
  kind: 'message.created' | 'message.recalled' | 'member.joined' | 'member.left' | 'owner.transferred' | 'read.cursor.advanced'
  happenedAt: string
  message?: Message
  roomId?: string
  rawSnapshot?: Record<string, unknown>
}
