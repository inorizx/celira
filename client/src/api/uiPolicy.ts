// UI 策略纯逻辑（**与宿主无关**；TASK_57 从 `hostUi.ts` 搬出来）。
//
// 这里放的是"界面该显示什么、该不该给入口、该不该给早反馈"的**可测试纯函数**：
// 能力探测、附件入口策略、契约字段的前端早校验、会话行展示投影。
// 无 React、无 DOM、无持久化依赖，也不接触任何凭据 —— 测试用哨兵证明这一点。
//
// 为什么从 `hostUi.ts` 搬出来：这些判断的**输入**（`ApiError.code`、`Room` 投影、
// 契约字段）在 Host 与 core 两条线上是同一份形状，调用点也大多不在 host 分支里。
// 留在带 `host` 名字的文件里会让人以为它们属于 Host 模式（并会在 Host 删除刀里
// 被误删）。

import type { Message, Participant, Room } from './domain'

// ── 能力探测 ────────────────────────────────────────────────────────────────
//
// **唯一判据 = 传输层真的实现了那个方法**（`WorldApi` 的四个事实域里，这些方法都是可选的）。
// 页面据此决定整块渲不渲染 —— 没有实现就不给一个点了必然报错/空着的区块，
// 也**不**为各模式维护第二份能力清单。
//
// ⚠️ **不要用 `mode` 当判据**（TASK_72 ⑥）：`ConnectionMode` 现在只有一个取值
// `'core'`，而**演示通道也活在 `mode === 'core'` 里**（登录页"不连接，先看演示界面"
// 走 `setPrefs('core','')`）⇒ 拿 mode 判"这台后端有没有这个能力"，在演示态会
// **照样按 core 渲染**（空区、点了报错）。问的也不是"是不是真实世界"
// （那是 `isRealWorldSession` 的事），而是"这台后端有没有这个能力"。

/** 传输层有没有这个能力：**方法真的在**才算（非函数、null、undefined 一律不算）。 */
function has(api: unknown, method: string): boolean {
  const carrier = api as Record<string, unknown> | null | undefined
  return typeof carrier?.[method] === 'function'
}

/**
 * Agent 撤销能力的**唯一判据**：传输层真的实现了 `revokeAgent`。
 *
 * core 直连（`CoreApi`）实现了；mock 兜底没有。
 * 页面据此决定是否渲染撤销区——没有实现就不给一个点了必然报错的按钮，
 * 也不为各模式维护第二份能力清单（判据与 `avatarSupported` 同一律）。
 */
export function supportsAgentRevoke(api: unknown): boolean {
  return has(api, 'revokeAgent')
}

/** **后端绑定**能力的唯一判据：`listBackendBindings`（绑定区的数据面）。 */
export function supportsAgentBindings(api: unknown): boolean {
  return has(api, 'listBackendBindings')
}

/** **凭据**能力的唯一判据：`listCredentials`（凭据区的数据面）。 */
export function supportsAgentCredentials(api: unknown): boolean {
  return has(api, 'listCredentials')
}

/**
 * **接入的「写」能力**：创建与撤销**都在**。
 *
 * ⚠ 2026-09-26 立：**读得到 ≠ 改得动**。原先页面只探了**读面**
 * （`listBackendBindings` / `listCredentials`），而按钮走的是**写面** ——
 * 演示通道（`MockApi` 有读无写）里四个按钮（创建 / 撤销接入、签发 / 撤销凭证）
 * **全都渲染出来、点下去什么也不发生、也不报错**（`store` 那几支当时是静默 `return`）。
 *
 * 两个判据分开是有意的：**看得到、改不动**是正当状态（只读投影），
 * 只是按钮不该出现。
 */
export function supportsAgentBindingWrite(api: unknown): boolean {
  return has(api, 'createBackendBinding') && has(api, 'revokeBackendBinding')
}

/** **凭据的「写」能力**：签发与撤销**都在**（理由同 `supportsAgentBindingWrite`）。 */
export function supportsAgentCredentialWrite(api: unknown): boolean {
  return has(api, 'issueCredential') && has(api, 'revokeCredential')
}

/**
 * ⚠ **2026-09-28**：这里原有 `supportsAgentServiceWrite`（Agent 服务的「写」能力
 * 探测：发布 / 授予 / 撤销**都在**才算能改）。它随 **Agent 服务层整体退役**
 * 删除 —— 判据的**形状**（"写面要么全在、要么当只读"）没有变，只是那一族方法
 * 连同它们的页面一起退场了；同类判据仍在下面（`supportsAgentBindingWrite` /
 * `supportsAgentCredentialWrite` / `supportsAgentProfile` 等）。
 */

/** Agent **详情**能力的唯一判据：`getAgent`（选中它时要不要去拉详情）。 */
export function supportsAgentDetail(api: unknown): boolean {
  return has(api, 'getAgent')
}

/** Agent **公共资料**能力的唯一判据：`getAgentProfile`。 */
export function supportsAgentProfile(api: unknown): boolean {
  return has(api, 'getAgentProfile')
}

/**
 * **按 Human 身份发联系人请求**的唯一判据：`sendContactRequest`。
 *
 * 它同时决定那句提示准不准：「直接输入对方 Human 的 UUID」只在**这台后端真按
 * Human 身份收请求**时才成立（没有这个能力的传输层上，那句话是误导）。
 */
export function supportsContactRequestByHumanId(api: unknown): boolean {
  return has(api, 'sendContactRequest')
}

/**
 * **真实建房**的唯一判据：`createHumanDirectRoom` 与 `createGroup` **都在**。
 *
 * 两条路都要在才走真实分支：「新的会话」那张表按选择分岔到私聊 / 群聊，缺一条时
 * 用 fixtures 那条路更诚实（演示通道 `getApi()` 是 null ⇒ 这一条为 false ⇒ 走 fixtures）。
 */
export function supportsRoomCreation(api: unknown): boolean {
  return has(api, 'createHumanDirectRoom') && has(api, 'createGroup')
}

/** **成员呈报**的唯一判据：`getMemberPresentations`（房间详情抽屉据此决定要不要去拉）。 */
export function supportsMemberPresentations(api: unknown): boolean {
  return has(api, 'getMemberPresentations')
}

/**
 * 这张资料卡该给哪几个**关系动作**（§「关系可逆性」）。
 *
 * 为什么是个纯函数而不是散在页面里的 `&&`：这三条判据各自都有一句理由，写进页面就只剩
 * 三个布尔表达式，谁也不知道哪条能删。
 *
 * - `remove`（断交）= **现在真有关系段**。契约把"本来就没有 active 段"也答成幂等 204，
 *   所以技术上随时能调；但给一个已经没有关系的人画「断交」，等于给用户一个**按了什么都不
 *   发生**的按钮。判据用「是不是联系人」而不是"服务端会不会拒绝"。
 * - `unblock`（解除拉黑）= **他在我的黑名单里**。这是唯一一个需要读状态才能判的。
 * - `block`（拉黑）= **他不在我的黑名单里**。**刻意不看 `isContact`**：契约允许拉黑任意
 *   active 主体（拉黑 ⊇ 解除），所以"一个还没加过我、上来就发申请的人"也拦得住——
 *   那恰恰是最需要这个按钮的场合。已经拉黑过的人不再画「拉黑」（画了就是第二个空按钮）。
 *
 * ⚠ **不在这里判「能不能对自己名下的 Agent 用」**：主人 2026-09-29 已裁——Agent 与人是
 * 平级主体，唯一区别是注册方式（人是自己注册、Agent 凭人的凭证注册），ownership 与关系
 * 是两件事。所以一条判据走到底，不分 kind、不分是不是自己带进来的。
 */
export function relationActionAvailability(input: {
  /** 这个人现在是不是我的联系人（accepted 段）。 */
  isContact: boolean
  /** 我拉黑过他吗（在 `GET /v1/blocks` 那一份里）。 */
  blocked: boolean
}): { remove: boolean; block: boolean; unblock: boolean } {
  return {
    remove: input.isContact,
    block: !input.blocked,
    unblock: input.blocked,
  }
}

/**
 * **Agent 模型区**的唯一判据：供应商/模型目录的数据面 `listProviders` 真的在。
 *
 * 这一区（当前模型那一行 ＋ 点进去的选择器）读的全是**供应商目录**那份事实。而那份事实
 * **今天在系统里根本不存在**：im 契约没有 provider/model 端点，Agent 详情投影
 * （`contracts.ts` 的 `AgentView`）里也没有模型字段 —— 两处都实测过。
 * ⇒ 真实世界里露出来的只可能是 fixtures 的假模型名与假供应商表。
 *
 * ⇒ 没有这个数据面就**整块不渲染**，与同页 `supportsAgentRevoke` /
 * `supportsAgentBindings` / `supportsAgentCredentials` 同一律：不给一个点了必然报错、
 * 或指向假数据的区块。哪天传输层真的接了目录，这一区自己就回来了 —— 判据是**探测**，
 * 不为各模式维护第二份能力清单，也不拿 `mode` 当门（TASK_72 ⑥）。
 */
export function supportsAgentModels(api: unknown): boolean {
  return has(api, 'listProviders')
}

// ── 附件入口策略 ────────────────────────────────────────────────────────────

/** 附件入口策略：传输层缺失/换代/未连接时必须 fail closed。
 *
 * TASK_63 起签名里只剩 `apiMode`：`hostOnly` 与 `hostStatus` 两个入参是 Host
 * 那条线的 gating（Host 输入区不支持附件上传、要等同源会话就绪），Host 全托管
 * 路径退役后**没有任何调用点会传 true / 传得到 host 会话** ⇒ 一并删掉，不留
 * 死参数（"没声明即只读"那类兜底默认值不跨层搬用，这里同理）。 */
export type AttachmentInputMode = 'real' | 'demo' | 'unavailable'

/**
 * 附件入口策略：**按能力探，不按 `mode`**。
 *
 * ⚠ 2026-09-26 修：这里原先是 `if (apiMode === 'core') return 'real'` —— 而
 * **演示通道的 `MockApi` 也声明 `mode = 'core'`**（`mock.ts`；`ConnectionMode` 如今
 * 只剩 `'core'` 一个取值，所以 `mode` 这一格**不携带任何信息**）。于是演示通道被判成
 * `'real'`：输入区只渲染真实上传项，而 `MockApi.uploadAttachment` 是**抛错**的
 * ⇒ 选文件必报「上传失败，请重试」，演示附件那条路**整条死掉**。
 *
 * 这正是仓库自己修过的那个错换个地方复发（`App.tsx` 的 `HostRouteGuard`：
 * 把「这是不是真实世界」错写成「这是不是某个 mode」）。
 *
 * 判据改成问**这一层有没有字节通道**（`fetchAttachmentBlob`）：
 * - `CoreApi` 有 ⇒ `'real'`（真实上传 + 授权 Blob 下载）；
 * - `MockApi` **不实现**它（演示附件的 `url` 直指 `/mock/*.svg`，由 `attachmentSource`
 *   直出）⇒ `'demo'`；
 * - 连传输层都没有（`null`）⇒ `'unavailable'`（fail closed，入口不渲染）。
 */
export function attachmentInputMode(api: unknown): AttachmentInputMode {
  if (api == null) return 'unavailable'
  return has(api, 'fetchAttachmentBlob') ? 'real' : 'demo'
}

// ── 契约字段的前端早校验 ────────────────────────────────────────────────────
//
// CORE_V1_PARTICIPANT_PRESENTATION.md §5/§7：在提交前按契约做 UTF-8 **字节**长度
// 校验与 NUL 拒绝，给早反馈（Core 仍是唯一权威裁决，本轮保存仍以 Core 的
// 400/409 为准）。空串归一化为 null 由表单提交处处理（本模块只判非法值）。
// 绝不接触、绝不输出任何 key 或凭据。

const utf8ByteLength = (value: string): number => new TextEncoder().encode(value).length

function containsNul(value: string): boolean {
  return value.includes('\u0000')
}

/** 群内 Presentation 字段非法反馈；合法返回 null。字段名用表单可见文案。 */
export function presentationFieldError(input: {
  roomDisplayName: string
  roomRoleName: string
  roomRoleDescription: string
}): string | null {
  if (
    containsNul(input.roomDisplayName) ||
    containsNul(input.roomRoleName) ||
    containsNul(input.roomRoleDescription)
  ) {
    return '内容包含不支持的字符（U+0000）'
  }
  if (utf8ByteLength(input.roomDisplayName) > 80) return `「在本群的称呼」超过 80 字节上限，请删减后保存`
  if (utf8ByteLength(input.roomRoleName) > 128) return `「职责角色名」超过 128 字节上限，请删减后保存`
  if (utf8ByteLength(input.roomRoleDescription) > 4096) return `「职责描述」超过 4096 字节上限，请删减后保存`
  return null
}

/** Agent 全局 Profile 字段非法反馈；合法返回 null。字段名用表单可见文案。 */
export function agentProfileFieldError(input: {
  shortDescription: string
  publicDescription: string
}): string | null {
  if (containsNul(input.shortDescription) || containsNul(input.publicDescription)) {
    return '内容包含不支持的字符（U+0000）'
  }
  if (utf8ByteLength(input.shortDescription) > 512) return `「一句话介绍」超过 512 字节上限，请删减后保存`
  if (utf8ByteLength(input.publicDescription) > 16384) return `「公开介绍」超过 16384 字节上限，请删减后保存`
  return null
}

// ── 房间对端名（模式无关的展示投影）─────────────────────────────────────────

/**
 * 房间真实对端参与者名（纯展示投影；只接受房间投影里的真实身份，
 * **绝不查 fixtures**）：优先 Direct/Human Direct 的 peer，其次 participants
 * 中的 agent；未知返回 undefined（调用方用泛化文案呈现，绝不冒充 Agent）。
 *
 * 原名叫 `hostPeerNameOf`；改名是因为它的输入 `Room.peer/participants` 在两条线上
 * 是同一份真实投影，**与宿主无关**。
 */
export function roomPeerNameOf(room: Pick<Room, 'peer' | 'participants'>): string | undefined {
  if (room.peer?.displayName) return room.peer.displayName
  const agent = room.participants?.find((p) => p.kind === 'agent')
  if (agent) return agent.displayName
  return undefined
}

// ── 会话行展示投影（display-only；绝不写回）─────────────────────────────────
//
// 真实模式下 `Room.lastMessagePreview/lastActivityAt` 可能来自静态快照（为空串），
// 而真实历史已加载进 messagesByRoom。本组函数从**已加载**的消息推导仅用于列表行
// 展示的投影，输出新 Room 对象，**不修改入参、不写 store、不写 Core、不写任何存储**。

/**
 * 从房间已加载消息推导会话行展示投影（纯函数）。
 *
 * - 取最新一条非 system 消息（messages 已是时间线顺序，最新在后）；
 * - preview 规则**只有这一份**（`store.refreshRooms` 直接调用本函数）：附件消息用
 *   `[图片]/[文件] 文件名`、正文取前 48 字符；作者名可解析时加
 *   `作者名：` 前缀（与 Mock/Core 列表行视觉一致，未知则省略）；
 * - `lastActivityAt`：当天显示 `sentAt`，否则显示 `day`；
 * - 没有任何可用消息（空数组或全是 system）时**原样返回入参**，绝不
 *   伪造「还没有消息」之外的任何内容；
 * - 返回新对象（`...room` 展开），入参永远不被变异。
 */
export function roomPreview(
  room: Room,
  messages: readonly Message[],
  authorNameOf: (authorId: string) => string | undefined = () => undefined,
): Room {
  const latest = [...messages].reverse().find((m) => !m.system)
  if (!latest) return room
  const preview =
    latest.attachments && latest.attachments.length > 0 && !latest.body
      ? `[${latest.attachments[0].kind === 'image' ? '图片' : '文件'}] ${latest.attachments[0].name}`
      : latest.body.slice(0, 48)
  const authorName = authorNameOf(latest.authorId)
  return {
    ...room,
    lastMessagePreview: authorName ? `${authorName}：${preview}` : preview,
    lastActivityAt: latest.day === '今天' ? latest.sentAt : (latest.day ?? ''),
  }
}

/**
 * 房间参与者展示名解析（纯函数，仅真实身份，绝不回退 fixtures）。
 *
 * - `me`（结构上等价 MeView）是当前登录的真实参与者；
 * - 只接受 `room.participants` / `room.peer` 中真实存在的作者；
 * - 未知 → `undefined`，由 `roomPreview` 省略作者前缀展示。
 */
export function roomAuthorNameOf(
  room: Room,
  me: { participant?: { id: string; display_name: string } | null } | null,
): (authorId: string) => string | undefined {
  return (authorId: string): string | undefined => {
    if (me?.participant && me.participant.id === authorId) return me.participant.display_name
    const member = room.participants?.find((p) => p.id === authorId)
    if (member) return member.displayName
    if (room.peer?.id === authorId) return room.peer.displayName
    return undefined
  }
}

/**
 * 批量会话行展示投影（纯函数）。
 *
 * 调用点**只有一处**：`store.refreshRooms`（列表渲染读的是它的结果，不直接调）。
 * ⚠ 在这条之前，`store.refreshRooms` 曾**内联重写**过同一规则，且已经与这里**行为分叉**
 * （内联版用 fixtures 的 `getParticipant` 解析作者名，未知作者会抛出、被 catch 吞掉整次
 * 刷新）。已收归到本条 —— 所以下面的每一项都应当被当成"**唯一那份**规则"来读。
 *
 * 每个房间复用 `roomPreview`（单一预览规则，不复制）；作者未知
 * 时 fail-soft（省略前缀）而绝不阻断投影，权威字段（尤其 unreadCount）
 * 原样保留、必然应用。返回全新 Room 对象，入参不被变异、不写
 * store/Core/任何存储。
 */
export function roomPreviews(
  rooms: readonly Room[],
  messagesOf: (roomId: string) => readonly Message[],
  me: { participant?: { id: string; display_name: string } | null } | null,
): Room[] {
  return rooms.map((r) => roomPreview(r, messagesOf(r.id), roomAuthorNameOf(r, me)))
}

// ── 建房 / 邀请的候选与走哪条路（纯逻辑）────────────────────────────────────

/**
 * 建房/邀请的候选集合：**accepted 联系人 ∪ 自有 Agent，按 id 去重**（先到先得，顺序不变），
 * 并**剔除已撤销的 Agent**。
 *
 * 为什么必须去重（2026-09-29 真机实测）：同一位 Agent 可以**同时**是 accepted 联系人与自有
 * Agent —— 屋主给自己的 Agent 发过好友申请并接受过，那是批次三允许的常态。直接拼两个数组会
 * 让它在「发起新会话」与「邀请成员」里各列出**两行同一个 id**（React key 也重复）。
 *
 * 为什么剔除已撤销：软撤销后参与者行仍在（契约：不硬删除、含 revoked 历史，
 * `GET /v1/agents` 就是"owner-scoped 管理视图，含 revoked 历史"），但它不再接收事件、
 * 房间成员关系与凭证都已作废 ⇒ 拿它建房/邀请是一条**必然失败的死选项**
 * （真机实测：已撤销的那支照样列在候选里）。
 *
 * 收成纯函数是为了让这条规则**可直测**：它是两份组件共用的那一份（原先各写一遍）。
 */
export function roomCandidates<T extends { id: string; revoked?: boolean }>(
  ...lists: readonly T[][]
): T[] {
  const byId = new Map<string, T>()
  for (const list of lists) {
    for (const item of list) {
      if (item.revoked) continue
      if (!byId.has(item.id)) byId.set(item.id, item)
    }
  }
  return [...byId.values()]
}

/**
 * 单选一个候选人建房时，该走**哪一条**建房路：判据是**它是什么**（`kind`），
 * 不是"它从哪个名单来"。
 *
 * ⚠ 2026-09-29 真机实测修：`NewRoomSheet` 原先先 `contacts.find(...)` 再
 * `ownedAgents.find(...)`，于是"既是联系人又是我的 Agent"的那一支**永远**走
 * `POST /v1/human-direct-rooms`（那条路只收 Human）⇒ 404、界面红字「找不到这位用户」，
 * 而正确的那条（回到你们那间已有的私人房）永远到不了。
 */
export function directRoomRouteFor(target: { kind: Participant['kind'] } | undefined): 'agent' | 'human' {
  return target?.kind === 'agent' ? 'agent' : 'human'
}

// ── 通话入口（`CORE_V1_CALL.md`，Call P0 窄解冻切片）────────────────────────

/**
 * **这一间房该给什么通话入口**（2026-10-09，主人当次判断：**"电话只人跟人"**）。
 *
 * 三条判据（都是"今天做得到什么"，不是"契约允许什么"——契约允许任意两个 active participant
 * 互呼，本函数**不改契约**，只管**界面给不给入口**）：
 *
 * 1. **对端是人**才给真入口：人的"在不在电话旁"由连接派生（Human 的 WS），响铃/接听/挂断
 *    对人是成立的动作；
 * 2. **对端是 Agent ⇒ 一个入口都不给**：实时语音要求**两端各有采集/播放**那一面，而执行端
 *    今天只会读事件、写文本消息（没有任何媒体面）⇒ 给了就是"按下去对面永远没人接"的假入口；
 *    而且"它在想 / 在调工具"那条红线说那些**不进 IM**，实时通话这条渠道里也没有位置放它们
 *    ⇒ 用户只会看到"它半天不出声"，而系统知道得比用户多；
 * 3. **群房不给**：P0 明确不做多方（契约 §6），"打给群里某个人"要另设计（成员选择 + 群内语义）。
 *
 * 视频那一格**一律是 `hint`**（今天不给真入口）：壳只放行麦克风，`Camera` 一律拒
 * （`app/src-tauri/src/lib.rs` 的 `on_permission_request` 三条边界）。
 */
export interface CallEntry {
  /** 语音：`'call'` = 真入口 / `'hint'` = 只给"尚未开放"的诚实提示 / `'none'` = 一个都不给。 */
  audio: 'call' | 'hint' | 'none'
  /** 视频：同上一格的三态（今天恒不是 `'call'`）。 */
  video: 'call' | 'hint' | 'none'
  /** 为什么是这个形状（登记与调试用；`audio === 'call'` 时是空串）。 */
  reason: string
}

export function callEntryFor(
  room: Pick<Room, 'kind' | 'peer'>,
  selfId: string,
): CallEntry {
  const peer = room.peer
  if (room.kind === 'group') {
    return { audio: 'none', video: 'none', reason: '群房不做多方通话（Call P0 只有两个参与者）' }
  }
  if (!peer || peer.id === selfId) {
    return { audio: 'none', video: 'none', reason: '这间房里没有可通话的对端' }
  }
  if (peer.kind === 'agent') {
    return {
      audio: 'none',
      video: 'none',
      reason: '对端是 Agent：实时语音是另一件事（它的执行端还没有采集/播放那一面）',
    }
  }
  return { audio: 'call', video: 'hint', reason: '' }
}


// ── 中性参与者（解析不出作者时的兜底展示）──────────────────────────────────

/**
 * 解析不出时用的中性参与者（**绝不回退 fixtures**）。
 *
 * ⚠ 这份实现原先在本目录下**有两份**（`MessageBubble` 与 `MessageContextMenu` 各一份、
 * 逐字相同）。收在这里，两边都改成 import 这一个 —— 两处实现迟早会打架。
 */
export function neutralParticipant(id: string): Participant {
  return {
    id,
    kind: 'human' as const,
    displayName: '参与者',
    initials: '?',
    signature: '',
    since: '',
  }
}

// ── 聊天首屏：消息到没到（**缺省不是事实**）────────────────────────────────

/**
 * 聊天首屏那一块的读数（**四态不许压成两态**）。
 *
 * 为什么单独立它：`messagesByRoom` 里"键不在"与"键在但是空数组"是**两件不同的事** ——
 * 前者是"还没读到 / 读失败"，后者才是"确认这个会话没有消息"。旧实现只有一句
 * `roomMessages.length === 0`，于是把三种不同的处境**都**渲染成
 * 「这个会话还没有消息，说一句开始吧。」：
 *   - 首屏还在路上 ⇒ 一句**暂时的假话**；
 *   - 首屏**读失败**（旧 store 是 `keep previous`，连错误都不留）⇒ 一句**永远挂着的假话**，
 *     看起来像历史被清空了；
 *   - 连接不在、根本没发起读取 ⇒ 同上。
 * 同一页里"更早的消息"反而有加载态（`olderLoading`）—— 首屏不该比它更差。
 *
 * 判据（顺序即优先级）：**到了** ⇒ `messages`（有消息）或 `empty`（确认没有）；
 * 没到 ⇒ 有失败人话就 `failed`（红路径，带重试），连接不可读就 `unreachable`，
 * 否则才是 `reading`。**只有 `empty` 那一支允许说"还没有消息"。**
 */
export type ChatFeedState =
  | { kind: 'messages' }
  | { kind: 'empty'; text: string }
  | { kind: 'reading'; text: string }
  | { kind: 'failed'; text: string }
  | { kind: 'unreachable'; text: string }

export function chatFeedState(input: {
  /** 这个房间的消息**到了没有**（store 里那个键在不在；空数组也算到了）。 */
  arrived: boolean
  /** 已经拿到的条数（`arrived` 且为 0 ⇒ 确认这个会话没有消息）。 */
  count: number
  /** 上一次读取失败的人话（`null`/缺省 = 没失败过）。 */
  error?: string | null
  /** 连接是否处于**可读取**状态（与 `loadRoomMessages` 的触发条件同一判据）。 */
  canRead: boolean
}): ChatFeedState {
  if (input.arrived) {
    if (input.count > 0) return { kind: 'messages' }
    return { kind: 'empty', text: '这个会话还没有消息，说一句开始吧。' }
  }
  if (input.error) return { kind: 'failed', text: `这个会话的消息没读到：${input.error}` }
  if (!input.canRead) return { kind: 'unreachable', text: '连接不在，这个会话的消息还没读到。' }
  return { kind: 'reading', text: '正在读取这个会话的消息…' }
}
