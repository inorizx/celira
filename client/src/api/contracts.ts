export type OpaqueCursor = string

export interface ApiErrorEnvelope {
  error: {
    code: string
    request_id: string
    message?: string
  }
}

// ── Call（CORE_V1_CALL.md，Call P0 窄解冻切片）──
// 点到点 WebRTC 实时协调：Call 状态机 + SDP/ICE 信令中继（进程内非持久）；
// 媒体由两端客户端 WebRTC P2P 直连，Core 不接触媒体字节；`media` 只是
// 振铃/UI 提示（Core 不解析媒体、不从 SDP 推断）。
//
// **2026-10-09：接线了**（原注释写着"尚未接入、这三个类型零消费者"，那一条**今天不成立**）：
// - 数据面 `api/core.ts` 实现了 `CallApi`（六个方法 ＋ `/v1/call/stream` 第二条 WS）；
// - 媒体面 `lib/callMedia.ts`（WebRTC P2P，复用 `lib/voice.ts` 的有界麦克风）；
// - 接线 `api/store.ts` 的 call slice（三个帧 ＋ 四个动作）。
// ⇒ 这三个类型**有了真实消费者**；`ChatHeader` 那两个入口的"尚未开放"提示在 UI 那一刀摘掉。
// ⚠ 仍然成立的边界：**Core 不碰媒体**（媒体面在两端之间）、**信令不落盘**（进程内、可过期）、
//   以及**视频面今天不开**（壳只放行麦克风，Camera 一律拒 —— 见 `app/src-tauri/src/lib.rs`
//   那一处 `on_permission_request` 的三条边界）。

// ── 通话（`CORE_V1_CALL.md` 窄解冻切片）─────────────────────────────────────
// 镜像的来源是**已实现**的那一侧：`im/server/src/adapters/http/call_routes.rs`（路由）与
// `im/server/src/protocol/call.rs`（请求/响应与 WS 帧的逐字段形状）。

/** 一条 Call 的公共投影（与 Core `CallView` 一一对应，白名单）。 */
export interface CallView {
  call_id: string
  room_id: string
  caller_id: string
  caller_display: string
  callee_id: string
  callee_display: string
  media: 'audio' | 'video'
  state: 'ringing' | 'in_progress' | 'ended'
  ended_reason: string | null
  created_at: string
  accepted_at: string | null
  ended_at: string | null
}

export interface CallIceCandidate {
  candidate: string
  sdp_mid: string | null
  sdp_mline_index: number | null
}

/** Call-stream WS 帧（事件名 = `type`；`data` 原帧 JSON）。 */
export interface CallStreamEvent {
  type: 'call.ring' | 'call.signal' | 'call.state'
  call?: CallView
  call_id?: string
  from_id?: string
  kind?: 'offer' | 'answer' | 'ice'
  payload?: { sdp?: string; candidates?: CallIceCandidate[] }
}

/** `POST /v1/calls` 请求体（`InitiateRequest`，服务端 `deny_unknown_fields`；`media` 缺省 audio）。 */
export interface CallInitiateRequest {
  room_id: string
  callee_id: string
  media?: 'audio' | 'video'
}

/**
 * `POST /v1/calls` 响应（`InitiateResponse`）：**201 = 新建**，**200 = 复用既有 active**
 * （同 (room, caller, callee) 已有 ringing/in_progress ⇒ 不重复振铃，靠既有返回做无状态幂等）。
 * ⇒ 调用方必须读 `created`，不许把"复用"当成"我又发了一次"。
 */
export interface CallInitiateResponse {
  call: CallView
  created: boolean
  request_id: string
}

/** `GET /v1/calls/{id}` 与三个动作（accept / decline / hangup）的响应（`CallResponse` / `CallActionResponse`）。 */
export interface CallEnvelope {
  call: CallView
  request_id: string
}

/**
 * `POST /v1/calls/{id}/signal` 请求体（`SignalRequest`）。
 *
 * ⚠ 边界是**服务端的**，客户端按它分批/截断（`lib/callMedia.ts` 的 `MAX_ICE_PER_SIGNAL`）：
 * offer/answer 的 `sdp` 是 `1..=16384` UTF-8 bytes 且**禁 U+0000**；ice 的 `candidates` 是
 * `1..=32` 条、每条 `1..=2048` bytes。越界 ⇒ 400 `call_payload_invalid`。
 */
export interface CallSignalRequest {
  kind: 'offer' | 'answer' | 'ice'
  sdp?: string
  candidates?: CallIceCandidate[]
}

export interface MeView {
  participant: {
    id: string
    kind: string
    display_name: string
    status: string
    created_at: string
  }
  session: {
    id: string
    created_at: string
  }
  request_id: string
}

// ── 世界身份（`GET /v1/home`）──────────────────────────────────────────────
// ⚠ **2026-09-26 清理**：这一段原先还挂着 `PairingGrant*` / `HomeSession*` 五个类型，
// 它们对应的是 `GET /api/v1/local/home/pairing-grants` 与 `/sessions` —— **Host BFF 的路径**。
// Host 全托管线 2026-09-20 退役、同日删除刀执行完毕（`ConnectionMode` 只剩 `'core'`，
// 由 `scripts/check-no-host.mjs` 守着不让它复生）⇒ 那五个类型**永不可能有消费者**，已删。
// 同一段注释原引 `docs/client/LOCAL_CLIENT_HOST.md` §3 —— **那份文档本身也已删除**
// （按 AGENTS.md 的权威文档顺序，它现在只作历史记录存在于 Git 里）。

/**
 * `GET /v1/home` → 这个世界的身份（**认证 Human**；非秘密，可展示）。
 *
 * 它回答的是"我连的是**哪个**世界"：`home_id` 是**世界级持久事实的归档键**
 * （客户端按它给事件 cursor / 系统消息 / 房间偏好分键，见 `api/worldIdentity.ts`）。
 *
 * ⚠ 这一格的来历：字段形状最早是给**已退役**的 Host BFF（`GET /api/v1/local/home`）
 * 定的；Host 线 2026-09-20 退役后它一直没有消费者。2026-09-25 起它对应的是
 * **core 直连**的 `GET /v1/home`（`im/server/src/adapters/http/home_routes.rs`），
 * 两者 wire 形状一致（`home_id` / `display_name` / `created_at`）。
 */
export interface HomeView {
  home_id: string
  display_name: string | null
  created_at: string
}

export interface RoomView {
  id: string
  kind: string
  status: string
  name: string | null
  created_at: string
}

export interface ParticipantMinView {
  id: string
  kind: string
  display_name: string
  status: string
}

export interface ReadStateView {
  last_read_message_id: string | null
  unread_count: number
}

/**
 * 「对方读没读我最后那条」（契约 `CORE_V1_READ_RECEIPT.md` §2）。
 *
 * **两态**；`null` / 缺字段 ＝ **问题不成立**（这一格不出现）——房不是 `direct`、
 * 对方当前不是这间房的 active 成员、或我在这间房还没说过话。
 * ⚠ 它是**读位置**，不是"对方看到了"：呈现文案不许写成后者。
 */
export type PeerReadView = 'read' | 'unread'


export interface RoomListEntryView {
  room: RoomView
  membership: {
    membership_id: string
    participant_id: string
    role: string
    joined_at: string
    left_at: string | null
    closed_by: string | null
  }
  peer: ParticipantMinView | null
  read_state: ReadStateView
  peer_read?: PeerReadView | null
}

export interface RoomListEnvelope {
  rooms: RoomListEntryView[]
}

export interface MemberView {
  participant_id: string
  kind: 'human' | 'agent'
  display_name: string
  role: 'owner' | 'member'
  joined_at: string
  left_at?: string | null
}

export interface AttachmentView {
  id: string
  file_name: string
  content_type: string
  size_bytes: number
}

export interface UploadAttachmentView {
  id: string
  file_name: string
  content_type: string
  size_bytes: number
  state: 'pending'
  uploaded_at?: string
}

export interface UploadAttachmentResponse {
  attachment: UploadAttachmentView
  request_id: string
}

export interface MessageActorView {
  id: string
  kind: string
  display_name: string
  status: string
}

export interface MessageView {
  id: string
  room_id: string
  actor: MessageActorView
  body: string | null
  mentions: string[]
  attachments: AttachmentView[]
  reply_to_message_id: string | null
  created_at: string
  recalled_at: string | null
}

export interface MessageHistoryEnvelope {
  messages: MessageView[]
  next_before: string | null
}

export interface SendMessageResponse {
  message: MessageView
  idempotency_replayed: boolean
  request_id: string
}

export type HumanEventKind =
  | 'message.created'
  | 'member.joined'
  | 'member.left'
  | 'owner.transferred'
  | 'read.cursor.advanced'
  | 'message.recalled'

export interface HumanEventEnvelope {
  id: string
  kind: HumanEventKind
  room_id: string
  snapshot: Record<string, unknown>
  created_at: string
}

export interface HumanEventsEnvelope {
  events: HumanEventEnvelope[]
  next_after: string
  request_id: string
}

export interface RecallResponse {
  message_id: string
  room_id: string
  recalled_at: string
  idempotency_replayed: boolean
  request_id: string
}

export interface ContactView {
  peer: ParticipantMinView
  status: 'pending' | 'accepted'
  /**
   * 相对**我**的方向：`incoming` = 对方向我发起（只有我能接受/拒绝）；
   * `outgoing` = 我发起的（等对方）。批次三第三刀补：主体做决定需要它
   * （在此之前，pending 的接受按钮对每一条都画，分不清哪条真该我答）。
   */
  direction: 'incoming' | 'outgoing'
  created_at: string
  accepted_at: string | null
}

export interface ListContactsResponse {
  contacts: ContactView[]
  request_id: string
}

export interface CreateContactResponse {
  contact: ContactView
  idempotency_replayed: boolean
  request_id: string
}

/**
 * 一条**我拉黑的主体**（`GET /v1/blocks` 的一行，契约 §5「黑名单」）。
 *
 * 只列**我作为 blocker** 的 active 拉黑段；内部 `block_id` 不在投影里（客户端也不需要它）。
 * 对端自带 `kind` —— 拉黑与关系同轴，两端可以是 Human 或 Agent。
 *
 * ⚠ 它是**单方面**的事实：这里没有、也不可能有"谁拉黑了我"——契约明写拉黑不泄露
 * （被拦的申请返回与「目标不存在」同形的 404）。客户端**不要把"申请失败"解释成"被拉黑"**。
 */
export interface BlockView {
  peer: ParticipantMinView
  created_at: string
}

export interface ListBlocksResponse {
  blocks: BlockView[]
  request_id: string
}

export interface CreateRoomResponse {
  room: RoomView
  memberships: unknown[]
  idempotency_replayed: boolean
  request_id: string
}

export interface MembershipView {
  membership_id: string
  participant_id: string
  role: 'owner' | 'member'
  joined_at: string
  left_at: string | null
  closed_by: string | null
}

/**
 * 主体进行态（契约 `CORE_V1_AGENT_PROTOCOL.md` §6）：一间房的**成员**名单
 * （人与 Agent 同形），每行只有两个易失投影。
 *
 * ⚠ 这里**没有**执行面：没有阶段（thinking/working/…）、没有生成流 id、
 * 没有 revision、没有草稿正文、没有 instance id——2026-09-28 主人裁决
 * 「不需要看到执行过程」后，那一整套随旧 Live State 删除。
 *
 * `online` = 这个主体此刻还有活着的连接（Human 的 WS / Agent 的门铃流或
 * 最近被看见）；`typing` = 它此刻正在这间房起草一条消息（几秒过期）。
 * 两者都**不是**耐久事实：读不到就是读不到，不要缓存成"它在线"。
 */
export interface RoomLiveMember {
  participant_id: string
  display_name: string
  kind: 'human' | 'agent'
  online: boolean
  typing: boolean
}

export interface RoomLiveSnapshot {
  room_id: string
  members: RoomLiveMember[]
}

/** Pairing Grant 的用途：设备配对（会话归签发者）或邀请一个人（归新人）。 */
export type GrantKind = 'self_device' | 'new_human'

/** 一张已签发的 Grant（不含 token——它只在签发那一刻出现一次）。 */
export interface PairingGrantView {
  grant_id: string
  status: 'active' | 'consumed' | 'revoked' | 'expired'
  grants: GrantKind
  device_label: string | null
  issued_at: string
  expires_at: string
  consumed_at: string | null
  revoked_at: string | null
}

export interface PairingGrantIssue {
  grant_id: string
  status: string
  grants: GrantKind
  device_label: string | null
  issued_at: string
  expires_at: string
  /** 一次性：这一串**只在这次响应里**出现，之后拿不回来。 */
  token: string
}

export interface PairingGrantListEnvelope {
  grants: PairingGrantView[]
}

/** 邀请码兑换的结果：属于**被邀请者自己**的一支新会话。 */
export interface InviteExchangeResult {
  session: { id: string; human_id: string; created_at: string }
  token: string
  home: { home_id: string; display_name: string | null; created_at: string }
}

export interface RoomMembersItem {
  membership: MembershipView
  participant: ParticipantMinView
}

export interface RoomMembersEnvelope {
  members: RoomMembersItem[]
  request_id: string
}

export interface InviteMemberResponse {
  membership: MembershipView
  idempotency_replayed: boolean
  request_id: string
}

export interface TransferOwnerResponse {
  membership: MembershipView
  idempotency_replayed: boolean
  request_id: string
}

export interface AgentView {
  id: string
  kind: string
  display_name: string
  status: string
  created_at: string
  revoked_at: string | null
}

export interface OwnershipView {
  agent_id: string
  human_id: string
  status: string
  created_at: string
  revoked_at: string | null
}

export interface GetAgentResponse {
  agent: AgentView
  ownership: OwnershipView
  request_id: string
}

/**
 * Profile 上的头像**公开投影**：只有存在性与展示所需的两个读数
 * （`CORE_V1_AVATAR_P0.md` §3.3）。
 *
 * storage key / SHA-256 / 路径绝不下行 —— 与消息附件同一卫生纪律。
 * 字节本身要另走 `GET /v1/rooms/{room_id}/members/{participant_id}/avatar`
 * （Room 成员授权路径），**不在这条投影里**。
 */
export interface ProfileAvatarView {
  content_type: string
  size_bytes: number
}

export interface ProfileView {
  participant_id: string
  profile_version: number
  short_description: string | null
  public_description: string | null
  /**
   * `null` ＝ 没有头像。
   *
   * 它**允许缺字段**（`avatar?:`）：头像切片晚于 Presentation P0 落地，早先的
   * Core 版本不发这个字段，而"没有头像"与"这台 Core 还没有这个字段"在展示上是
   * 同一件事（都画 fallback）。把它当成解析失败，等于把一次版本错位升级成
   * "个人资料读不出来"。消费方对缺字段与 `null` 一视同仁。
   */
  avatar?: ProfileAvatarView | null
  published_by_id: string
  published_by_kind: string
  published_at: string
}

export interface GetProfileResponse {
  agent: AgentView
  ownership: OwnershipView
  profile: ProfileView | null
  request_id: string
}

/**
 * `GET /v1/me/profile`：**我自己**的名字与呈现（表达权第四刀，2026-10-17）。
 *
 * ⚠ 两件事**并排**给出来，不是"合起来的 profile 对象"：`display_name` 是既有
 * Participant 身份事实（`participants.display_name`），`profile` 是呈现版本链上的一格
 * （头像引用 + 版本 + 署名）。契约原文就拒绝复制 `display_name` 进版本链。
 *
 * `profile === null` = **从未写过呈现**（不是空对象、不是错误）。
 */
export interface GetMyProfileResponse {
  display_name: string
  profile: ProfileView | null
  request_id: string
}

export interface PutProfileResponse {
  profile: ProfileView
  idempotency_replayed: boolean
  request_id: string
}

export interface GetRoomResponse {
  room: RoomView
  membership: MembershipView
  peer: ParticipantMinView | null
  read_state: ReadStateView
  peer_read?: PeerReadView | null
  request_id: string
}

export interface BackendBindingView {
  id: string
  agent_id: string
  created_by_human_id: string
  label: string
  status: string
  created_at: string
  revoked_at: string | null
}

export interface CreateBindingResponse {
  binding: BackendBindingView
  idempotency_replayed: boolean
  request_id: string
}

// ── Group Room Presentation（CORE_V1_PARTICIPANT_PRESENTATION.md §2.2/§7）──
// 公共呈现事实：Room 可见性授权读取；仅当前 group room 的 active owner 可写，
// 目标必须是该 room 的 active Agent 成员。写入必须携带 Idempotency-Key；
// 新 key 时 base_version 必须等于当前最新版本，否则 409 room_presentation_stale。
// `published_by_kind` 是派生投影（来源 published_by_id -> participants.kind），
// 绝非第二个持久身份事实。

/** member-presentations 摘要单项：participant 最小资料 + Profile
 * short_description + **avatar 存在性** + Presentation 的 room_display_name /
 * room_role_name / room_role_description_preview（读时刻从 full stored 描述生成，
 * 最大 512 字节；绝不含 public_description / full role description）。
 *
 * ⚠ **2026-09-26 补 `avatar`**：服务端（`protocol/participant_presentation.rs` 的
 * `MemberPresentationSummaryItem`）一直有这个字段，**是这份白名单投影漏了它** ——
 * 于是房间成员列表拿不到"这个人有没有头像"，只能一律画 fallback。
 * 形状与 `ProfileView.avatar` 同一个（`AvatarView` ⇒ `ProfileAvatarView`），
 * 字节本身照旧另走 `GET /v1/rooms/{room_id}/members/{participant_id}/avatar`。
 * 缺字段的口径也照抄那一条：**"没有头像"与"这台 Core 还没有这个字段"在展示上是同一件事**。 */
export interface MemberPresentationSummaryItem {
  participant: ParticipantMinView
  short_description: string | null
  avatar?: ProfileAvatarView | null
  room_display_name: string | null
  room_role_name: string | null
  room_role_description_preview: string | null
}

/** GET /v1/rooms/{room_id}/member-presentations 响应。 */
export interface GetMemberPresentationsResponse {
  members: MemberPresentationSummaryItem[]
  request_id: string
}

/** Room Presentation 版本视图。 */
export interface PresentationView {
  room_id: string
  participant_id: string
  presentation_version: number
  room_display_name: string | null
  room_role_name: string | null
  room_role_description: string | null
  published_by_id: string
  published_by_kind: string
  published_at: string
}

/** GET /v1/rooms/{room_id}/members/{participant_id}/presentation 响应。 */
export interface GetPresentationResponse {
  participant: ParticipantMinView
  presentation: PresentationView | null
  request_id: string
}

/** PUT /v1/rooms/{room_id}/members/{participant_id}/presentation 响应。 */
export interface PutPresentationResponse {
  presentation: PresentationView
  idempotency_replayed: boolean
  request_id: string
}

export interface ListBindingsResponse {
  bindings: BackendBindingView[]
  request_id: string
}

export interface CredentialView {
  id: string
  binding_id: string
  status: string
  created_at: string
  revoked_at: string | null
}

export interface IssueCredentialResponse {
  credential: CredentialView
  token: string | null
  idempotency_replayed: boolean
  request_id: string
}

export interface ListCredentialsResponse {
  credentials: CredentialView[]
  request_id: string
}

