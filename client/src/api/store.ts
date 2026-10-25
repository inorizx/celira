import { create } from 'zustand'
import { useShallow } from 'zustand/react/shallow'
import { CoreApi } from './core'
import { MockApi } from './mock'
import { newIdempotencyKey } from './client.ts'
import { applyRoomPage, createInFlightGuard, mergeOlderPage, settleOptimisticId } from './asyncOwnership'
import { emitBroadcast, subscribeBroadcast } from './broadcast'
import { loadNotifyPrefs, showMessageNotification } from './notifications'
import { clearRoomCache, loadRoomCache, loadRoomsCache, saveRoomCache, saveRoomsCache } from './idb'
import type { AppEvent, CallApi, WorldApi } from './types'
import type { CallStreamEvent, CallView } from './contracts'
import type {
  Attachment,
  BlockedPeer,
  ContactRequestReceipt,
  DeliveryState,
  Message,
  OwnedAgent,
  Participant,
  Room,
  RoomAttention,
} from '@/api/domain'
import type {
  BackendBindingView,
  CredentialView,
  GetPresentationResponse,
  MeView,
  MemberPresentationSummaryItem,
  RoomLiveSnapshot,
  RoomMembersItem,
} from './contracts'
import {
  registerParticipantLookup,
  registerFixtureFallbackGate,
  getParticipant,
} from './participantDirectory'
// 参与者投影的**唯一权威**（原先本文件与 core.ts 各有一份逐字相同的副本）。
import { toParticipant } from './participantProjection.ts'
import { mentionedMe } from './mentionTargets.ts'
// 通知门控的**唯一裁决面**（纯函数）：这里只负责凑事实，不在 `if` 串里自己裁决。
import { shouldNotifyMessage } from './notificationGate.ts'
// 提醒档的投影（乐观更新 / 回退 / 回读共用一份判据）。
import { DEFAULT_ROOM_ATTENTION, withRoomAttention } from './roomAttention.ts'
import { credentialScopeKey } from './credentials.ts'
import { errorText } from './contractErrors.ts'
import { createCallMedia, type CallMedia, type CallMediaState, type CallSignalKind, type CallSignalPayload } from '@/lib/callMedia'
import { CALL_MEDIA_FAILURE_TEXT, callBlockedReason, callEndedNotice } from '@/lib/callText'
import { roomPreviews } from './uiPolicy'
// 「把它接到这台机器上」到壳的那一问（首刀步骤 2）。没有壳时 `startAnjia` 回 `bridge-missing`，
// 页面据此走那两句诚实标注（`CREATE_WONT_REPLY`）——`hasAnjiaBridge` 是唯一判据（判内部注入，
// 不判平台名，承 `shell.ts`）。
import { hasAnjiaBridge, startAnjia, type AnjiaResult } from './anjia.ts'
import { roomPresentationErrorMessage } from './presentation'
import { applyRoomProfile } from './roomProfile.ts'
import {
  DEFAULT_PREFS,
  PREFS_KEY,
  isRealWorldSession,
  parsePrefs,
  serializePrefs,
  type ConnectionMode,
  type ConnectionPrefs,
} from './prefs'
import {
  SYS_KEY_NAME,
  WORLD_NAMESPACE,
  worldScopedKey,
  parseWorldIdentity,
  worldKeyOf,
} from './worldIdentity'
import {
  EMPTY_ROOM_PREFS,
  applyRoomPrefs,
  loadRoomPrefs,
  saveRoomPrefs,
  toggleId,
  type RoomPrefs,
} from './roomPrefs'
import { legacyKeysUnder, legacyRemove, readThrough, storage } from './storage'

const CURSOR_KEY_NAME = 'cursor'

/**
 * **世界级持久事实的归档键**（2026-09-25 立，2026-09-29 并入统一命名空间）。
 *
 * 事件 cursor / 系统消息 / 房间缓存都是**世界级**的：它们的键该是**世界的身份**
 * （`GET /v1/home` 的 `home_id`），不是**凭据**（token）—— 备份/恢复出来的第二个世界
 * 可能带着同一支 token，按凭据分键会让两个世界互读。拿不到世界身份时**退回凭据**
 * （老服务器、离线、演示、还没实现这一面的服务端）：行为与从前一字不差。
 *
 * ⚠ **这个"退回凭据"是有代价的，2026-09-26 记账**：退回时 token 会以**键名**的形式
 * 落进世界级键（`celira.world:<token>:cursor` 等），也会作为**字段值**落进 IndexedDB 的
 * 房间缓存（`idb.ts` 的 `worldKey`）。⇒ **登出必须把整族都清掉**（见 `clearWorldScopedKeys`）。
 *
 * 残留（已知、未改）：世界身份拿不到时，token 仍会作为 IDB 的字段值存在。它的**边际暴露为零**
 * （同一个源下 `celira.connection.prefs` 本来就存着那支 token），但确实与
 * `DESIGN_SYSTEM.md`「凭据不得写入任何其它持久层」的字面相抵触。改法是**把退回值换成
 * 不可逆摘要**——那是要主人裁的取舍，本刀不动。
 */
function worldKey(): string {
  return worldKeyOf(prefs.worldId, prefs.token)
}

/**
 * **登出时清掉所有"按世界分键"的持久事实**（2026-09-26 立，2026-09-29 改为清整族）。
 *
 * 原先按**三个精确键名**删（`<前缀>:<key>` 与裸前缀两种形状都要试）。命名空间统一之后
 * 判据反而简单了：这个世界的世界级键**全部**是 `celira.world:<key>:` 开头的，
 * **按前缀整族清**即可 —— 不会再漏（从前漏过的正是"只删 cursor 一个"）。
 *
 * 登出时 token 仍以键名形式残留这一问题，随本刀一并消失：清的是**前缀**，不是某个精确键。
 */
async function clearWorldScopedKeys(key: string): Promise<void> {
  const scope = String(key ?? '').trim()
  const prefix = `${WORLD_NAMESPACE}:${scope}:`
  // ① **真正的那一份**：窄口（壳里 = 家里的文件存储）。从前这里是同步读 `localStorage` 的，
  //    于是在壳里**登出根本没清掉家里那一族**（cursor / 系统消息 / 房间缓存都还在文件里）——
  //    2026-10-08 两段式启动那一刀顺手修掉：清的就是窄口。
  try {
    for (const k of await storage.keys(prefix)) await storage.remove(k)
  } catch {
    /* 清不掉就清不掉：位置与缓存都不是事实（与从前同一条纪律） */
  }
  // ② **老位置**（webview 的 `localStorage`）：升级上来时可能还留着一份 —— **只清不读**
  //    （唯一读它的地方是搬家读，判据在 `storage.ts` 的 `readThrough`）。
  legacyRemove(legacyKeysUnder(prefix))
}

function omitKey<V>(map: Record<string, V>, key: string): Record<string, V> {
  const next = { ...map }
  delete next[key]
  return next
}

function sysStorageKey(key: string): string {
  return worldScopedKey(key, SYS_KEY_NAME)
}

/**
 * 系统消息的读写（**两处都走窄口，异步**）。
 *
 * ⚠ 2026-10-08 存储收尾时更正了一句陈旧的注释：这里从前写着"这两条还是同步的"，
 * 但代码在同一刀里已经改成 `await storage.read` 了 —— 现在**没有同步读存储的调用点**。
 * 它们仍在 `set()` 回调之外被 `void …then(…)` 串起来（`set()` 的回调必须同步，
 * 而"系统消息落盘"不是渲染路径上的事）。
 */
async function loadSysMessages(roomId: string, key: string): Promise<Message[]> {
  const raw = await storage.read(sysStorageKey(key))
  try {
    const all = JSON.parse(raw ?? '{}') as Record<string, Message[]>
    return all[roomId] ?? []
  } catch {
    return []
  }
}

/** 写入系统消息。写不进去返回 `false`（不抛）—— 缓存丢了只是重来一次，不是故障。 */
async function saveSysMessages(roomId: string, key: string, messages: Message[]): Promise<boolean> {
  let all: Record<string, Message[]> = {}
  try {
    all = JSON.parse((await storage.read(sysStorageKey(key))) ?? '{}') as Record<string, Message[]>
  } catch {
    all = {}
  }
  all[roomId] = messages
  return storage.write(sysStorageKey(key), JSON.stringify(all))
}

/**
 * 读 persisted prefs：**全部委托 prefs.ts**（新字段带默认值、旧数据开箱可用）。
 *
 * ⚠ 走的是**窄口**（异步）—— 于是壳里它落在 `<app_config_dir>/local-store.json`，
 * 浏览器里还是 `localStorage`（`api/storage.ts` 的缺省实现）。
 *
 * 老位置（webview 的 `localStorage`）里那一格只在窄口还空着的那一次被读：读到就**读穿迁移**
 * （写进窄口）—— 判据与那一步写在 `storage.ts` 的 `readThrough` 里（**一处**，各族共用）。
 */
async function loadPrefs(): Promise<ConnectionPrefs> {
  return parsePrefs(await readThrough(PREFS_KEY))
}

/**
 * 写 persisted prefs（**异步，走同一个窄口** —— 与上面那份读是同一个权威）。
 *
 * 写失败**不抛、也不静默**：返回 `false` 让调用方有机会记一笔。凭据与服务器地址是
 * 用户下次开应用要用的东西，写不进去值得让调用方知道（调用点目前只记日志）。
 */
async function savePrefs(next: ConnectionPrefs): Promise<boolean> {
  return storage.write(PREFS_KEY, serializePrefs(next))
}

/**
 * **两段式启动的第二段：补水**（2026-10-08 立，同日第二刀收口）。
 *
 * 第一段是模块求值 —— 那时只能用**缺省值**（窄口是异步的，模块求值期拿不到文件）。
 * 所以真正的持久值在 `createRoot(...).render(...)` **之前**由这里读进来：
 * 用户看到的**第一帧**就已经是他自己的连接（不闪、不先落登录页再跳回来）。
 *
 * ⚠ 顺序：**先装配窄口**（`installShellStorage`）、再补水 —— 补水要经过那条窄口。
 * ⚠ 失败一律**降级成缺省值**，绝不拦着渲染（白屏比"没读到偏好"糟得多）。
 * ⚠ **谁用这个 store 谁先补水**：正门是 `main.tsx`（挂载之前）。要模拟"启动"的测试也得调它
 *   （见 `tests/reconnect-compensation.test.ts`）—— 不调的话，模块求值期的缺省值会一直用下去，
 *   落在盘上的世界级键（游标等）就找不到，表现为"读回来是空的"。
 * ⚠ **幂等**：补两次没事（正门与测试各补一次也不会互相踩）。
 * ⚠ 本函数只管**与世界身份有关的那两族**（连接偏好、房间用户侧偏好）：后者的键按世界分，
 *   所以必须在连接偏好之后读。纯外观与草稿那三族与世界无关，各有自己的补水入口
 *   （`lib/themePrefs.ts` / `lib/userTheme.ts` / `lib/wallpaper.ts` / `lib/messageDrafts.ts`），
 *   由 `main.tsx` 一并在挂载之前调 —— 它们漏了不会有人替它们兜住，所以那边有一条机械守卫
 *   （`tests/local-state.test.ts`：搬家读的调用点必须在这里点名过）。
 */
let hydrated: Promise<void> | null = null

export function hydrateLocalState(): Promise<void> {
  hydrated ??= (async () => {
    try {
      prefs = await loadPrefs()
    } catch (error) {
      prefs = DEFAULT_PREFS
      console.error('[prefs] 补水失败，这次按缺省值启动（下次启动再试）', error)
    }
    // 房间的用户侧偏好（置顶 / 免打扰）也在这一族的顺序里：它的键按**世界**分
    // （`celira.world:<世界>:room-prefs`）⇒ 必须等连接偏好读完才知道读哪一个世界。
    // 2026-10-08 第二刀起它走同一个窄口（从前是本模块求值期/此处的一次同步 `localStorage` 读）。
    try {
      roomPrefs = await loadRoomPrefs(worldKeyOf(prefs.worldId, prefs.token))
    } catch (error) {
      roomPrefs = EMPTY_ROOM_PREFS
      console.error('[roomPrefs] 补水失败，这次按"没意见"启动（下次启动再试）', error)
    }
    // ⚠⚠ **必须把值推回 store 的状态**：页面判"这是不是真实世界"（`isRealWorldSession`）读的是
    //   store 的 `token`，不是这个模块变量。只改模块变量的话，用户明明带着凭据启动，页面却按
    //   **演示通道**渲染（fixtures 漏出来、机器页读不到）—— 2026-10-08 实测踩到过：
    //   `verify:ui` 从 91/91 掉到 67/80，就是少了这一句。形状与 `prefs.changed` 那条广播一致。
    useAppStore.setState({
      mode: prefs.mode,
      token: prefs.token,
      serverUrl: prefs.serverUrl,
      worldId: prefs.worldId,
    })
  })()
  return hydrated
}

export type ConnectionState = 'idle' | 'connecting' | 'connected' | 'error' | 'reconnecting'

interface AppState {
  mode: ConnectionMode
  token: string
  /** 直连 Core 的服务器地址；空串 = 同源（页面自身来源的 `/v1`）。 */
  serverUrl: string
  /**
   * **当前连接的世界**（`GET /v1/home` 的 `home_id`；空串 = 还不知道 / 问不到）。
   *
   * 它与 `token` 是两件事：token 回答"我是谁"，它回答"我在哪个世界" —— 世界级持久事实
   * （事件 cursor / 系统消息 / 房间缓存）按它归档（见 `api/worldIdentity.ts`）。
   */
  worldId: string
  connection: ConnectionState
  connectionError?: string
  /** 启动探测是否仍进行中：定案完成前不得渲染 Splash/Mock。 */
  startupPending: boolean
  /**
   * 正门判定结果：手里没有可直接复用的直连凭据 ⇒ 第一屏就是登录页
   * （服务器地址 + token）。
   */
  needsLogin: boolean
  me: MeView | null
  rooms: Room[]
  messagesByRoom: Record<string, Message[]>
  /**
   * 每个房间**首屏读取**的失败人话（没有这个键 = 没失败过）。
   *
   * 为什么必须有它：`messagesByRoom` 里"键不在"与"键在但是空数组"是**两件不同的事**
   * —— 前者是"还没读到"（或读失败），后者才是"确认这个会话没有消息"。旧实现把两者
   * 都渲染成「这个会话还没有消息，说一句开始吧。」：读失败时那句话会**永远挂着**，
   * 看起来像历史被清空了。这个映射是"读失败"那一格的唯一权威。
   */
  messagesLoadErrorByRoom: Record<string, string>
  nextBeforeByRoom: Record<string, string | null>
  /**
   * **这间房的时间线窗口读到过没有**（读到 = 一整页连续时间线落下来了）。
   *
   * 为什么必须有这一格（2026-10-20 真机事故）：`messagesByRoom` 有两个写者 ——
   * 「整页」（`listMessages` / 更早页 / 缓存种子）与「单条直播间事件」。后者写进去的是
   * **片段**，不是时间线；而"键在"从前同时表示这两种东西 ⇒ 开房落页时那**一条**被当成
   * 时间线的**开头**，最新那条被顶到最上面（小可 11:29 那条跑到 01:34 那一段之前）。
   *
   * 有这一格以后：**键在且 `timelineLoadedByRoom` 在 ⇒ 连续窗口**（可以合并）；
   * **键在但这一格不在 ⇒ 只有片段**（落页时以服务端那一页为准）。判据与两条合并
   * 函数的分工写在 `asyncOwnership.ts` 的 `applyRoomPage` 里。
   */
  timelineLoadedByRoom: Record<string, true>
  /**
   * **当前这一通电话**（`ringing` / `in_progress`；没有就是 `null`）。
   *
   * ⚠ 它**不是持久事实**：Call 在 Core 里是进程内投影，重启即失忆（`CORE_V1_CALL.md` §2）
   * ⇒ 客户端也**一个字都不落盘**（刷新页面就是"这通电话没了"），与"通话记录"这种
   * 持久事实划清界限（契约 §6 明确不做通话记录）。
   */
  call: CallView | null
  /** 我在这通里的角色（由 `caller_id` 与我自己的 id 比出来；没有通话时 `null`）。 */
  callRole: 'caller' | 'callee' | null
  /** 媒体面（WebRTC P2P）的连接态：UI 用它说"正在连接 / 已接通"。 */
  callMediaState: CallMediaState
  /**
   * 这一格最后一次**要说给用户听**的话（结束原因 / 失败人话），`null` = 没有。
   *
   * ⚠ 与 `call` 分开：`call` 在 `ended` 之后就清空了（它不再是"当前这一通"），
   * 而"对方拒接了"这句话得留在屏幕上让用户看见。
   */
  callNotice: string | null
  contacts: Participant[]
  pendingContacts: Participant[]
  /**
   * 我拉黑过的主体（`GET /v1/blocks`）。
   *
   * ⚠ **只有"我拉黑的"这一半**：契约明写拉黑不泄露，不存在"谁拉黑了我"的读取面。
   * 界面上也不要把"申请失败"翻译成"被拉黑"——那是客户端自己编的事实。
   */
  blocks: BlockedPeer[]
  ownedAgents: OwnedAgent[]
  /** Owner Agent 发现的易失三态：读取失败绝不伪装成真实空列表。 */
  ownedAgentsLoading: boolean
  ownedAgentsError?: string
  membersByRoom: Record<string, RoomMembersItem[]>
  /**
   * 主体进行态快照，按房存（`GET /v1/rooms/{id}/live` 的最近一次读数）。
   * **易失投影**：只用于显示"谁在线 / 谁正在写"，绝不参与任何持久判断，
   * 也不进 IndexedDB 缓存（读不到就是读不到）。
   */
  roomLiveByRoom: Record<string, RoomLiveSnapshot>
  /** Group Room Presentation 摘要投影（core 模式，打开群成员抽屉按需读取；
   *  mock 无此契约，绝不把演示字段冒充真实公开事实）。 */
  memberPresentationsByRoom: Record<string, MemberPresentationSummaryItem[]>
  bindingsByAgent: Record<string, BackendBindingView[]>
  credentialsByBinding: Record<string, CredentialView[]>
  /**
   * **「把它接到这台机器上」的读数**，按 Agent 存（首刀步骤 2）。
   *
   * **易失**：只记这一次会话里壳报回来的结论（进程号 / 环境根 / 日志路径——只是报告，
   * 壳不做 PID 认领）。**不进任何持久层**：一台机器上到底有没有它的家，权威在**那台机器**
   * （壳每问一次都自己重判），不在这个浏览器的记忆里。
   */
  anjiaByAgent: Record<string, AnjiaResult>

  /** `serverUrl` 省略 = 保留当前地址（切换模式不应抹掉用户填过的服务器）。 */
  setPrefs: (mode: ConnectionMode, token: string, serverUrl?: string) => void
  setStartupPending: (pending: boolean) => void
  setNeedsLogin: (needsLogin: boolean) => void
  connect: () => Promise<void>
  disconnect: () => void
  retryConnection: () => Promise<void>
  loadRoomMessages: (roomId: string) => Promise<void>
  loadOlder: (roomId: string) => Promise<number>
  refreshContacts: () => Promise<void>
  /** 发一段关系申请；回执带着「对方是谁」与「这条在等谁」（见 `ContactRequestReceipt`）。 */
  sendContactRequest: (participantId: string, idempotencyKey?: string) => Promise<ContactRequestReceipt>
  acceptContact: (humanId: string) => Promise<void>
  /** 拒绝给我的那条 pending 申请（只有 `direction === 'incoming'` 的才该有入口）。 */
  declineContact: (participantId: string) => Promise<void>
  /**
   * 断交。⚠ 会**同时关掉那间房**（契约：段结束时双方成员周期同事务关闭）——
   * 调用方必须先把"永久封存、复合另建"这件事告诉用户。
   */
  removeContact: (participantId: string) => Promise<void>
  /** 拉黑。契约里 **拉黑 ⊇ 解除**，所以它的后果与 `removeContact` 相同，另加进黑名单。 */
  blockParticipant: (participantId: string) => Promise<void>
  /** 解除拉黑（**不恢复关系**）。 */
  unblockParticipant: (participantId: string) => Promise<void>
  /** 重读黑名单（建连时与每次拉黑/解除之后各一次）。 */
  refreshBlocks: () => Promise<void>
  createHumanDirectRoom: (humanId: string) => Promise<string>
  createDirectRoom: (agentId: string) => Promise<string>
  createGroup: (
    name: string,
    /**
     * 每个「逻辑建群」的 Idempotency-Key（**可选**参数；core 自己生成一份，
     * 忽略调用方传的）。key 绝不入持久层/DOM/日志。
     */
    idempotencyKey?: string,
  ) => Promise<string>
  createAgent: (displayName: string, idempotencyKey?: string) => Promise<string>
  /**
   * 叫壳把一支 Agent 接到**这台机器**上（首刀步骤 2）。
   *
   * 没有壳（浏览器）⇒ 壳不会假装：`startAnjia` 直接回 `bridge-missing`，
   * 页面据此说「到桌面版里做这件事」——**读不到 ≠ 没接上**。
   */
  attachAgent: (agentId: string) => Promise<AnjiaResult>
  refreshOwnedAgents: () => Promise<void>
  refreshAgent: (agentId: string) => Promise<void>
  refreshRoom: (roomId: string) => Promise<void>
  /**
   * 保存**群简介**（`PUT /v1/rooms/{room_id}/profile`，契约
   * `CORE_V1_ROOM_PROFILE.md` §3）。
   *
   * 失败原样抛出（带 Core 稳定分类码）：UI 据此走 `contractErrors` 的人话，
   * 409 那一支还要触发权威重读。成功之后 store 自己 `refreshRoom` ——
   * 版本号只认 Core 的读面，绝不在本地 +1。
   */
  updateRoomProfile: (
    roomId: string,
    input: { baseVersion: number; description: string },
  ) => Promise<void>
  refreshRoomMembers: (roomId: string) => Promise<void>
  /**
   * 刷新主体进行态快照（谁在线 / 谁正在写）。易失投影，失败保留上一次读到的
   * 值（不编造、不清空成"全员离线"）。
   */
  refreshRoomLive: (roomId: string) => Promise<void>
  /**
   * 声明「我正在这间房写」（best-effort，永不抛）。节流由调用方（输入框）做，
   * store 只负责发出去。
   */
  notifyTyping: (roomId: string) => Promise<void>
  // ── 通话（`CORE_V1_CALL.md`，Call P0 窄解冻切片）────────────────────────
  //
  // 判据全在这一组动作里（页面只调它们，不许自己拼信令）：
  // `startCall` 发起、`acceptCall` 接听、`declineCall` 拒接、`hangupCall` 挂断。
  // ⚠ 能力判据只有一处：`hasCallApi(getApi())` —— 演示通道没有通话（见 `types.ts` 的 `CallApi`）。
  /** 发起一通（audio）。已在通话里 / 房间里没有对端 / 这个世界没有通话面 ⇒ 只留一句人话。 */
  startCall: (roomId: string, calleeId: string) => Promise<void>
  /** 接听**当前这一通**（只有 callee 能接；没有来电时什么都不做）。 */
  acceptCall: () => Promise<void>
  /** 拒接当前这一通（callee）。 */
  declineCall: () => Promise<void>
  /** 挂断当前这一通（任一方）。 */
  hangupCall: () => Promise<void>
  /** 把"这一格最后说了什么"清掉（UI 关掉提示时调）。 */
  clearCallNotice: () => void
  refreshMemberPresentations: (roomId: string) => Promise<void>
  getRoomPresentation: (
    roomId: string,
    participantId: string,
  ) => Promise<GetPresentationResponse>
  updateRoomPresentation: (
    roomId: string,
    participantId: string,
    input: {
      baseVersion: number
      roomDisplayName: string | null
      roomRoleName: string | null
      roomRoleDescription: string | null
    },
    /**
     * 每个「逻辑保存」的 Idempotency-Key（**可选**参数；core 自己生成一份，
     * 忽略调用方传的，客户端不缓存版本、不预判结果）。key 绝不入持久层/DOM/日志。
     */
    idempotencyKey?: string,
  ) => Promise<void>
  inviteMember: (
    roomId: string,
    participantId: string,
    /**
     * 每个「逻辑邀请」的 Idempotency-Key（**可选**参数；core 自己生成一份，
     * 忽略调用方传的）。key 绝不入持久层/DOM/日志。
     */
    idempotencyKey?: string,
  ) => Promise<void>
  removeMember: (roomId: string, participantId: string) => Promise<void>
  leaveRoom: (roomId: string) => Promise<void>
  transferOwnership: (
    roomId: string,
    participantId: string,
    /**
     * 每个「逻辑转让」的 Idempotency-Key（**可选**参数；core 自己生成一份，
     * 忽略调用方传的）。key 绝不入持久层/DOM/日志。
     */
    idempotencyKey?: string,
  ) => Promise<void>
  revokeAgent: (agentId: string) => Promise<void>
  revokeSession: () => Promise<void>
  refreshBindings: (agentId: string) => Promise<void>
  createBackendBinding: (agentId: string, label: string) => Promise<void>
  revokeBackendBinding: (agentId: string, bindingId: string) => Promise<void>
  refreshCredentials: (agentId: string, bindingId: string) => Promise<void>
  issueCredential: (agentId: string, bindingId: string, replacesCredentialId: string | null) => Promise<string | null>
  revokeCredential: (agentId: string, bindingId: string, credentialId: string) => Promise<void>
  send: (
    roomId: string,
    input: {
      body?: string
      replyToId?: string
      attachments?: Attachment[]
      /** 显式提及（agent Participant ID 列表；仅 group Room 非空）。 */
      mentions?: string[]
    },
  ) => Promise<void>
  recall: (roomId: string, messageId: string) => Promise<void>
  /**
   * 重发一条**发送失败**的消息。
   *
   * ⚠ 它**复用首次那把 Idempotency-Key**（存在那条 pending 的 `idempotencyKey` 上）：
   * 契约 §6.3 规定同 actor + 同 key + 同 payload ⇒ 返回**原** Message（200，
   * `idempotency_replayed=true`）。所以"首次其实已落库、只是响应丢了"这种情况下重试
   * **不会**产生第二条公共消息 —— 那是架构红线「绝不因重放重复公共消息」要的形状。
   */
  retry: (messageId: string) => Promise<void>
  markRoomRead: (roomId: string) => Promise<void>
  /**
   * 置顶 / 免打扰：**用户侧偏好**，只写本机存储（`api/storage.ts` 窄口）（`api/roomPrefs.ts`），
   * 不经过世界侧传输层、不发任何请求。
   */
  togglePinned: (roomId: string) => Promise<void>
  toggleMuted: (roomId: string) => Promise<void>
  /**
   * **服务端提醒档**（`PUT /v1/rooms/{room_id}/attention`，契约 `CORE_V1_IM_CONTRACT.md` §9.1）：
   * `all` = 普通群消息也提醒我；`mentions` = 只接受**结构化** @。
   *
   * ⚠ 与上面两个**不是同一层**：`pinned` / `muted` 是本机偏好（`api/roomPrefs.ts`，
   * 不发请求），这一格是**世界侧的个人事实**（换设备跟着走）。
   *
   * 形状：**乐观更新**（先改房间列表，开关立刻响应）→ 发请求 → 用服务端回读值落回；
   * 失败**回退到原值并重抛**（不许静默成"已保存"，也不许让开关自己弹回去还说没事）。
   */
  setRoomAttention: (roomId: string, level: RoomAttention) => Promise<void>
  /**
   * 改完自己的名字之后，把**世界的回读值**落进 store（表达权第四刀，2026-10-17）。
   *
   * 为什么必须有它：`me` 是世界侧事实的投影 —— 页面**不能**自己改本地那一份
   * （那就是第二份事实 ✗）。服务端 `PUT /v1/me/profile` 的回读值才是权威，
   * 落进来即可；名字的权威永远在世界上。
   */
  applyMyName: (displayName: string) => void
  refreshRooms: () => Promise<void>
  pollEvents: () => Promise<void>
}

/**
 * 当前的世界侧传输层（`CoreApi`；演示通道是 `null`）。
 *
 * 类型是**四个事实域的合取**（`WorldApi`，见 `types.ts` 头部裁决）——原先叫 `ChatApi`，
 * 那个 62 方法的上帝接口已拆开。**家平面不在这个变量里**：Agent 的运行/日子/模型走
 * `useHomeEntries` 与 `api/homes.ts`，与连接状态无关。
 */
let api: WorldApi | null = null

export function getApi(): WorldApi | null {
  return api
}

// ── 通话那一格的**模块级**东西（不是 React 状态，也不该进 store 的持久面）──────
//
// 为什么媒体对象不放进 store：它是**命令式资源**（有 peer connection、有麦克风轨道、
// 有关闭时机），塞进 zustand 的不可变状态里只会得到"状态里躺着一个活对象"这种形状。
// store 记的是它的**态**（`callMediaState`），对象本身放这里，只有本文件能碰。

/** 当前这一通的媒体面（没有通话 / 已经结束 ⇒ `null`）。 */
let activeMedia: CallMedia | null = null

/**
 * **能力探测的唯一判据**：这个世界有没有通话面（`CallApi`）。
 *
 * 为什么不并进 `WorldApi`：演示通道（`MockApi`）没有通话 —— 它不是"事实少了半边"，
 * 而是这一格今天只有真实世界有（见 `types.ts` 的 `CallApi` 那段）。页面不许自己
 * `as any` 或按"有没有壳"之类的旁证去猜，只认这一处。
 */
export function callApiOf(candidate: WorldApi | null): CallApi | null {
  if (!candidate) return null
  // ⚠ 这一处 `unknown` 转换是**故意的、而且只此一处**：`WorldApi` 是四个事实域的合取，
  //   通话是第五个（故意没并进去 —— 演示通道没有它，见 `types.ts` 的 `CallApi`）。
  //   探法就是"那个方法在不在"（与 `uiPolicy` 的能力探测同一条判据），而 TS 无法对
  //   "接口之外的额外方法"做静态证明 ⇒ 只能在**这一个函数**里转换，调用方拿到的仍是窄接口。
  const maybe = candidate as unknown as Partial<CallApi>
  return typeof maybe.initiateCall === 'function' && typeof maybe.subscribeCallStream === 'function'
    ? (candidate as unknown as CallApi)
    : null
}

/** 把一条信令发给对端（失败只记控制台：**信令发不出去不该把通话状态搞坏**）。 */
async function sendCallSignal(callId: string, kind: CallSignalKind, payload: CallSignalPayload): Promise<void> {
  const callApi = callApiOf(api)
  if (!callApi) return
  try {
    await callApi.signalCall(callId, { kind, ...payload })
  } catch (error) {
    console.warn('Celira：一条通话信令没发出去（原文只在这里）', error)
  }
}

/** 收尾：关媒体、清"当前这一通"、把结束原因留成一句人话。 */
function finishCall(
  set: (partial: Partial<AppState>) => void,
  call: CallView,
  selfId: string,
): void {
  activeMedia?.close()
  activeMedia = null
  set({ call: null, callRole: null, callMediaState: 'idle', callNotice: callEndedNotice(call, selfId) })
}

/**
 * Call-stream 的三个帧（**唯一**处理点；`parseCallStreamFrame` 已经把读不懂的丢掉）。
 *
 * 三格各自的判据：
 * - `call.ring`：**只有 callee 会收到**（契约 §4.1）⇒ 这就是"来电"。此时**不采集麦克风**
 *   （用户还没接），只建一个能收信令的媒体对象把 offer/ICE 缓冲起来；
 * - `call.state`：状态/结束原因变了 ⇒ `ended` 就收尾（关媒体 + 留一句话）；
 * - `call.signal`：交给媒体面（**它自己按角色筛方向**，见 `callMedia.ts`）。
 */
function handleCallFrame(
  set: (partial: Partial<AppState>) => void,
  frame: CallStreamEvent,
  selfId: string,
): void {
  if (frame.type === 'call.ring' && frame.call) {
    const call = frame.call
    activeMedia?.close()
    activeMedia = createCallMedia({
      role: 'callee',
      send: (kind, payload) => void sendCallSignal(call.call_id, kind, payload),
      onState: (mediaState) => set({ callMediaState: mediaState }),
      onFailure: (failure, detail) => {
        if (detail) console.warn('Celira：通话媒体面失败（原文只在这里）', failure, detail)
        set({ callNotice: CALL_MEDIA_FAILURE_TEXT[failure] })
      },
    })
    set({ call, callRole: 'callee', callMediaState: 'idle', callNotice: null })
    return
  }
  if (frame.type === 'call.state' && frame.call) {
    const call = frame.call
    if (call.state === 'ended') {
      finishCall(set, call, selfId)
      return
    }
    // 非终态：只更新状态（**不重置媒体面** —— 那会白白再采集一次麦克风）。
    set({ call })
    return
  }
  if (frame.type === 'call.signal' && frame.kind) {
    void activeMedia?.applySignal(frame.kind, frame.payload ?? {})
  }
}

/**
 * **这一轮有没有"真实世界的传输层"**（= 拿到的是 `CoreApi`，不是演示通道的 `MockApi`）。
 *
 * 为什么需要它、以及为什么不能用别的判据：
 *
 * - `getApi() === null` **不够**：演示通道 `connect()` 之后拿到的是 `MockApi`，
 *   那个判据在演示态**不成立**（2026-09-25 实测踩过：`useHomeEntries` 的闸门因此失效，
 *   演示通道照样去打 `/home-status` 并拿到一串 502）；
 * - `isRealWorldSession` 回答的是**另一个问题**（"手里有没有真实凭据"，见 `prefs.ts`），
 *   它是 fixtures 露不露面的权威判据 —— 拿它当"要不要读家"会混两层含义；
 * - 这一层要问的是**传输层是谁**，所以就直接问它：是不是 `CoreApi`。
 *   判据落在 `api/` 里（别处不必知道 `CoreApi` / `MockApi` 这两个具体类型）。
 *
 * 消费者：家平面那两族读取（`useHomeEntries` / `useHomeReading` / 模型配置页）——
 * 演示通道里家根本不存在（那台机器都没在跑），它们**一个请求都不该发**。
 */
export function hasRealWorldTransport(): boolean {
  return api instanceof CoreApi
}

const notifiedMessageIds = new Set<string>()
const initializedRooms = new Set<string>()

/**
 * 把**上一次已知的**会话行预览/时间，接到这一次从服务器拿回来的房间列表上。
 *
 * 为什么需要：`GET /v1/rooms` 不带预览与时间（`core.ts` 如实给空串），而 `refreshRooms` 是
 * **整表替换** ⇒ 一次不带历史的刷新（连接后第一次唤醒、`syncFromApi`）会把已经算出来的预览
 * 连同房间一起抹掉，界面退回一屏「还没有消息」，缓存里那份也被覆盖成空的
 * （2026-09-29 真机实测：IndexedDB 里三条房间的 preview 全是空串）。
 *
 * 规则只有一条：**服务器那份是空的、而我手里有一份** ⇒ 用我这份顶上；权威历史一到
 * （`roomPreviews` 按已加载消息重算）自然覆盖它。服务器给的非空值**永远优先**。
 */
function carryKnownPreviews(server: Room[], previous: readonly Room[]): Room[] {
  if (previous.length === 0) return server
  const prevById = new Map(previous.map((r) => [r.id, r]))
  return server.map((r) => {
    if (r.lastMessagePreview) return r
    const prev = prevById.get(r.id)
    if (!prev?.lastMessagePreview) return r
    return { ...r, lastMessagePreview: prev.lastMessagePreview, lastActivityAt: prev.lastActivityAt }
  })
}

/**
 * 用**本机上一次的房间缓存**给"还没有预览"的房间补上会话行预览与时间。
 *
 * 为什么需要（2026-09-29 真机实测）：`GET /v1/rooms` 不带预览/时间（`core.ts` 如实给空串），
 * 连接那一刻又没有任何房间的历史在手 ⇒ 回访用户打开应用看到的是一屏「还没有消息」，
 * 要点开每一间房才逐个补上。这里一次 IndexedDB 读把上次已知的预览顶上，**只补空的那些**
 * （已有预览的房间一字不动）；权威历史一到就被 `reprojectRoomPreviews` 覆盖。
 *
 * 缓存读不到 / 读坏了 ⇒ 原样返回入参（fail-soft：宁可没有预览，也不编一个）。
 */
async function seedPreviewsFromRoomsCache(rooms: Room[], key: string): Promise<Room[]> {
  if (rooms.length === 0 || rooms.every((r) => r.lastMessagePreview)) return rooms
  const cached = await loadRoomsCache(key)
  if (!cached || cached.length === 0) return rooms
  const byId = new Map(cached.map((r) => [r.id, r]))
  return rooms.map((r) => {
    if (r.lastMessagePreview) return r
    const c = byId.get(r.id)
    if (!c?.lastMessagePreview) return r
    return { ...r, lastMessagePreview: c.lastMessagePreview, lastActivityAt: c.lastActivityAt }
  })
}

/**
 * **只登记、不通知**：把这几条记进"见过"的集合。
 *
 * 谁走它：**翻历史**（`loadOlder` 拿回来的那一页是**旧的**，不是"新来的"）——
 * 登记之后，将来任何一次 diff（`trackNewMessages`）都不会把它们当成新消息**再通知一遍**
 * （边界上偶尔重逢的那几条也一并挡住）。
 */
function markMessagesSeen(messages: Message[]): void {
  for (const m of messages) notifiedMessageIds.add(m.id)
}

function trackNewMessages(roomId: string, messages: Message[]): Message[] {
  const newlyAdded: Message[] = []
  const wasInitialized = initializedRooms.has(roomId)
  for (const m of messages) {
    if (notifiedMessageIds.has(m.id)) continue
    notifiedMessageIds.add(m.id)
    if (wasInitialized && !m.system && m.authorId !== useAppStore.getState().me?.participant?.id) {
      newlyAdded.push(m)
    }
  }
  initializedRooms.add(roomId)
  return newlyAdded
}

/**
 * 这条消息要不要弹通知。
 *
 * **门控判据不在这里** —— `api/notificationGate.ts` 的 `shouldNotifyMessage` 是唯一裁决面
 * （全局开关 / 系统消息 / 自己发的 / 本机免打扰 / 服务端提醒档 / 提及偏好，顺序都在那一份里）。
 * 本函数只做两件事：**凑事实**（读开关偏好、查房间、判"是不是结构化 @ 我"），以及
 * 门控放行之后**决定弹什么**（标题里那句「（提及）」）。
 *
 * `attention` 读的是**当前房间列表**：它是 `GET /v1/rooms` 的投影（`core.toRoomListEntry`），
 * 乐观更新落在同一份 `rooms` 上 ⇒ 用户拨开关的那一刻，门控就已经按新档判了（不用等刷新）。
 * `undefined`（还没读回 / 旧服务端）由门控按 `all` 处理 —— 宁多响一声，不静默吞掉群消息。
 */
async function maybeNotify(roomId: string, message: Message): Promise<void> {
  const me = useAppStore.getState().me
  const notify = await loadNotifyPrefs()
  const room = useAppStore.getState().rooms.find((r) => r.id === roomId)
  // 提及判据读**结构化事实**（消息 `mentions` 里的 participant id），绝不从正文猜：
  // 正文里手打一句 `@我的显示名` 不构成提及（契约 `CORE_V1_MENTION_TARGETS.md` §6：
  // Core 永不从正文推断 mention）。旧判据是 `message.body.includes('@'+显示名)`，一次
  // 误报与一次漏报都只差一个名字的写法。
  const isMention = mentionedMe(message, me?.participant?.id)
  const allowed = shouldNotifyMessage({
    notificationsOn: notify.enabled,
    mentionsOnly: notify.mentions,
    // 本机「消息免打扰」= **完全静音**：它压过服务端提醒档（含 @ 我），见门控文件头 §优先级。
    muted: room?.muted === true,
    attention: room?.attention,
    system: message.system === true,
    fromSelf: message.authorId === me?.participant?.id,
    mentionedMe: isMention,
  })
  if (!allowed) return
  let authorName = '新消息'
  try {
    authorName = getParticipant(message.authorId).displayName
  } catch {
    // unknown author
  }
  const body = (isMention ? '提及了你：' : '') + (message.body || '[附件]').slice(0, 80)
  // 两种形态（浏览器 / 壳里的原生通知）由 `api/notifications.ts` 一处分流；这里是**唯一调用点**。
  await showMessageNotification({
    roomId,
    title: isMention ? `${authorName}（提及）` : authorName,
    body,
  })
}

const cacheWriteTimers = new Map<string, ReturnType<typeof setTimeout>>()

function scheduleCacheWrite(roomId: string, key: string, messages: Message[], nextBefore: string | null): void {
  // 守卫：这一次排队的写入必须**仍属于当前的世界**（换世界/退出登录后落地的旧写入一律丢弃）。
  if (key !== worldKey()) return
  const existing = cacheWriteTimers.get(roomId)
  if (existing) clearTimeout(existing)
  cacheWriteTimers.set(
    roomId,
    setTimeout(() => {
      cacheWriteTimers.delete(roomId)
      void saveRoomCache(roomId, key, messages, nextBefore)
    }, 500),
  )
}

/**
 * **第一段：模块求值期只有缺省值**（2026-10-08 两段式启动）。
 *
 * ⚠ 这里从前是 `let prefs = loadPrefs()`（**同步**读 `localStorage`）—— 那是"文件存储接不进来"
 * 的唯一原因：模块求值期读不了异步的东西。现在真正的值由 `hydrateLocalState()` 在
 * `createRoot` 之前补进来（见 `main.tsx`），这里只保证"**任何时刻都有值可用**"。
 */
let prefs: ConnectionPrefs = DEFAULT_PREFS
/**
 * **房间的用户侧偏好（置顶 / 免打扰）**，住本机存储（`api/storage.ts` 窄口），按世界分键。
 *
 * ⚠ 这两个字段的**唯一写入者**是下面的 `togglePinned` / `toggleMuted`，
 * **唯一读取投影**是 `refreshRooms` 末尾的 `applyRoomPrefs`。
 * 它们**不是公共事实**：Core 的 `GET /v1/rooms` 永远不带这两个字段，
 * 也永远不会带（用户侧偏好不进 im 服务端）。见 `api/roomPrefs.ts` 头部。
 *
 * ⚠ 与上面的 `prefs` 同款：模块求值期只能是空集，真值在补水时读（`hydrateLocalState`）。
 */
let roomPrefs: RoomPrefs = EMPTY_ROOM_PREFS
/**
 * 世界身份那一次问话（memo）——**模块级**：`connect()` 的闭包与跨标签页的处理器
 * 都要能作废它（换连接 = 换世界，见 `ensureWorldIdentity`）。
 */
let worldIdentity: Promise<string> | null = null

// 翻页用进程内普通每房间守卫：重复/并发的翻页只放行一次。
const olderLoadGuard = createInFlightGuard()

export const useAppStore = create<AppState>()((set, get) => {
  const mergeSelf = (rooms: Room[], me: MeView | null): Room[] => {
    if (!me) return rooms
    const self: Participant = toParticipant(me.participant)
    return rooms.map((r) => {
      if (r.participants?.some((p) => p.id === me.participant.id)) return r
      return {
        ...r,
        participants: r.participants ? [self, ...r.participants] : undefined,
        participantIds: r.participantIds.includes(me.participant.id)
          ? r.participantIds
          : [me.participant.id, ...r.participantIds],
      }
    })
  }

  const registerLookup = (entries: { room: Room; peer: Participant[] }[], me: MeView | null) => {
    // ⚠ 原判据 `api?.mode !== 'core'` —— `ConnectionMode` 只剩 `'core'` 一个取值
    // （`prefs.ts`），那个比较**恒假**；它实际起的作用是**空值守卫**（`api` 为 null 时
    // `undefined !== 'core'` 为真）。改写成它真正在说的那句话（2026-09-26，审计 D18）。
    if (!api) return
    const lookup = new Map<string, Participant>()
    if (me) {
      lookup.set(me.participant.id, toParticipant(me.participant))
    }
    for (const e of entries) {
      for (const p of e.peer) lookup.set(p.id, p)
    }
    registerParticipantLookup((id) => lookup.get(id))
  }

  const refreshRooms = async (): Promise<boolean> => {
    if (!api) return false
    const apiNow = api
    try {
      const entries = await apiNow.listRooms()
      const rooms = entries.map((e) => e.room)
      registerLookup(entries, get().me)
      const me = get().me
      const mergedRooms = mergeSelf(rooms, me)
      // 会话行展示投影**只有一份实现**（`uiPolicy.roomPreviews` —— 有测试的那一侧）。
      //
      // 这里原先**内联重写了一遍**同一规则，而且已经与那份**行为分叉**：内联用
      // `getParticipant` 解析作者名，而它遇到解析不出的作者会**抛异常** —— 异常被本函数
      // 的 `catch` 吞掉 ⇒ 一个解析不出的作者能让**整个房间列表刷新静默停掉**（正是本仓
      // "红路径不许静默"要治的形状）。`uiPolicy` 那侧是 fail-soft：作者未知就省略前缀、
      // 预览照给。用回它即修掉这条，且权威字段（尤其 unreadCount）原样保留。
      const projectedRooms = roomPreviews(
        // ⚠ **先接上"上一次已知的"预览**再投影：这一句是 2026-09-29 真机实测加的。
        // 少了它，一次"手里还没有历史"的刷新（连接后第一次唤醒 / `syncFromApi`）会把已经算出来
        // 的预览连同房间整表替换掉 ⇒ 界面退回一屏「还没有消息」，连缓存那份也被覆盖成空的。
        carryKnownPreviews(mergedRooms, get().rooms),
        (roomId) => get().messagesByRoom[roomId] ?? [],
        me,
      )
      set({
        // 用户侧偏好（置顶 / 免打扰）**投影在最后**：它们住本机存储（`api/storage.ts` 窄口），
        // 世界侧返回的 rooms 里没有这两个字段（真实世界那一份永远不带）。
        // 演示通道同样过这一道 —— 那时 roomPrefs 是空的，fixtures 自带的
        // `pinned` / `muted` 被原样保留（`applyRoomPrefs` 值没变就返回同一对象）。
        rooms: applyRoomPrefs(projectedRooms, roomPrefs),
      })
      // ⚠ 原先这两行都写着 `api.mode === 'core' && …` —— 恒真（`ConnectionMode` 只有一个
      // 取值），读的人却会以为这里存在一个模式分流。条件已去掉，行为一字未变（审计 D18）。
      if (!suppressBroadcast) emitBroadcast({ type: 'rooms.changed' })
      // ⚠ 存的是**投影后**的那一份（带预览与时间），不是线协议原样那一份：房间缓存的消费者
      // 有两处 —— "连不上时拿它顶上"（那一屏也该有预览）与 `seedPreviewsFromRoomsCache`
      // （首屏补预览）。存原样那份的话，这两个消费者都拿不到预览（2026-09-29 修）。
      void saveRoomsCache(worldKey(), projectedRooms)
      return true
    } catch {
      // transient; keep previous state
      return false
    }
  }

  /**
   * **就地重投影会话行**（不发网络请求）：只把当前 `rooms` 按当前 `messagesByRoom` 再过一次
   * 展示投影，规则仍是 `uiPolicy.roomPreviews` 那一份。
   *
   * 为什么需要它：`refreshRooms` 在**连接时**跑一次，那时任何房间的历史都还没加载 ⇒ 每一行
   * 的预览与时间都是空的（`core.ts` 的列表端点本来就不带这两个字段，如实给空串）。而
   * "进入房间 → 历史加载完成"过去**不会**再投影一次 ⇒ 时间线里明明有 5 条消息，左边那一行
   * 还挂着「还没有消息」，要等到别的事情（比如在另一间房发条消息）再跑一次 `refreshRooms`
   * 才补上。
   *
   * 这里只补**触发点**，不复制规则：有消息的房间重新算，没加载过的房间 `roomPreview` 原样
   * 返回入参（值没变 ⇒ `applyRoomPrefs` 也返回同一对象），不会把任何东西擦掉。
   */
  const reprojectRoomPreviews = () => {
    const me = get().me
    const projected = roomPreviews(get().rooms, (roomId) => get().messagesByRoom[roomId] ?? [], me)
    set({ rooms: applyRoomPrefs(projected, roomPrefs) })
    // 顺手把带预览的那一份落进房间缓存：下一次进这个应用时，
    // `seedPreviewsFromRoomsCache` 才有东西可顶（否则首屏仍旧是一屏「还没有消息」）。
    void saveRoomsCache(worldKey(), projected)
  }

  const refreshMessages = async () => {    if (!api) return
    const { messagesByRoom, connection } = get()
    const roomIds = Object.keys(messagesByRoom)
    if (roomIds.length === 0 || connection !== 'connected') return
    await Promise.all(
      roomIds.map(async (roomId) => {
        try {
          const { messages, nextBefore } = await api!.listMessages(roomId)
          const sys = await loadSysMessages(roomId, worldKey())
          // ⚠ **通知只吃"最新一页"**（`trackNewMessages` 是全局已通知集合）：翻出来的历史
          //   不许被当成新消息再通知一遍 ⇒ 这一份 diff 必须留在**服务端那一页**上。
          const added = trackNewMessages(roomId, [...sys, ...messages])
          scheduleCacheWrite(roomId, worldKey(), messages, nextBefore)
          set((s) => ({
            // ★ **合并，不是替换**（2026-10-16 主人真机反馈："加载更早消息…机制有问题？"）：
            //   从前这里把整条时间线换成"服务端最新一页"（50 条）⇒ `loadOlder` 翻出来的历史
            //   被下一条新消息整条抹掉、"加载更早"等于白加载；视图还会跳回去。
            //   判据三条（顺序不动 / 同 id 以服务端为准 / 新 id 追加）见 `mergeLatestPage`。
            // ⚠ 2026-10-20：这一页怎么落，先看**这间房的窗口读过没有**（`applyRoomPage`）——
            //   只有片段（直播间那条先到）时，合并会把最新那条顶到最上面。
            messagesByRoom: {
              ...s.messagesByRoom,
              [roomId]: applyRoomPage({
                existing: s.messagesByRoom[roomId] ?? [],
                page: [...sys, ...messages],
                loaded: s.timelineLoadedByRoom[roomId] === true,
              }),
            },
            timelineLoadedByRoom: { ...s.timelineLoadedByRoom, [roomId]: true },
            // ★ **游标不许被顶掉**：已经翻过历史时，`nextBeforeByRoom` 指的是"再往前那一页"，
            //   那是用户真正的下一条；拿最新一页的游标覆盖它 ⇒ 翻页重复/跳页、历史再也拉不回来。
            //   只有**还没有游标**（这一间第一次读）才用它初始化。
            nextBeforeByRoom:
              s.nextBeforeByRoom[roomId] === undefined || s.nextBeforeByRoom[roomId] === null
                ? { ...s.nextBeforeByRoom, [roomId]: nextBefore }
                : s.nextBeforeByRoom,
          }))
          // 通知不是渲染路径上的事：读开关偏好现在是异步的（`api/notifications.ts`），
          // 而 `set()` 的回调必须同步 —— 所以在这里 fire-and-forget。
          for (const m of added) void maybeNotify(roomId, m)
        } catch {
          // keep previous
        }
      }),
    )
  }

  const syncFromApi = () => {
    void refreshRooms()
    void refreshMessages()
  }

  /**
   * **补拉失败后的重试**（退避，与 WS 重连同款量级：2s 起、翻倍、封顶 30s）。
   *
   * 为什么必须有：唤醒只是提示，事件真身住在服务端的耐久账本里；游标不动保证
   * **不丢**，但「补拉」这一次失败（Core 重启中途 / 502 / 瞬断）之后必须**有人
   * 再来拉** —— 下一次唤醒可能永远不来（没有新事件就没有唤醒），于是断线窗口里
   * 的事件会被搁在服务端，界面永远缺那一段。契约 §10 的判据是「唤醒可全丢、
   * REST 恢复不能丢」：恢复自己也要能收敛。
   */
  let pollRetryTimer: ReturnType<typeof setTimeout> | null = null
  let pollRetryAttempt = 0

  const clearPollRetry = () => {
    if (pollRetryTimer) {
      clearTimeout(pollRetryTimer)
      pollRetryTimer = null
    }
    pollRetryAttempt = 0
  }

  const schedulePollRetry = () => {
    if (pollRetryTimer) return
    const delay = Math.min(30_000, 2_000 * 2 ** pollRetryAttempt)
    pollRetryAttempt += 1
    pollRetryTimer = setTimeout(() => {
      pollRetryTimer = null
      void pollEvents()
    }, delay)
  }

  const pollEvents = async () => {
    if (!api) return
    try {
      const cursor = (await storage.read(worldScopedKey(worldKey(), CURSOR_KEY_NAME))) ?? undefined
      const page = await api.listEvents(cursor)
      applyEvents(page.events)
      await storage.write(worldScopedKey(worldKey(), CURSOR_KEY_NAME), page.nextAfter)
      if (page.events.length > 0) void refreshRooms()
      // 补拉成功 ⇒ 退避复位（下一次失败重新从 2s 起算）。
      clearPollRetry()
    } catch {
      // 瞬时失败：游标没动（不丢），但必须排一支重试（见上）。
      schedulePollRetry()
    }
  }

  const applyEvents = (events: AppEvent[]) => {
    if (events.length === 0) return
    set((s) => {
      const messagesByRoom = { ...s.messagesByRoom }
      for (const ev of events) {
        if (ev.message && ev.roomId && ev.kind === 'message.created') {
          // ⚠ **这一支只许写"片段"，不许写"窗口"**（2026-10-20）：这间房没读过时间线时，
          //   `?? []` 会凭空造出 `[这一条]` —— 那是**片段**（会话列表的预览、通知要它），
          //   不是时间线的开头。所以这里**不许**碰 `timelineLoadedByRoom`；开房落页时
          //   `applyRoomPage` 会按"这一格不在"以服务端那一页为准。
          const list = messagesByRoom[ev.roomId] ?? []
          if (!list.some((m) => m.id === ev.message!.id)) {
            messagesByRoom[ev.roomId] = [...list, ev.message]
            // 同上：`set()` 回调里只能 fire-and-forget（读开关偏好是异步的）。
            void maybeNotify(ev.roomId, ev.message)
          }
        } else if (ev.kind === 'message.recalled' && ev.roomId && ev.message?.id) {
          const list = messagesByRoom[ev.roomId] ?? []
          messagesByRoom[ev.roomId] = list.map((m) =>
            m.id === ev.message!.id
              ? { ...m, system: true, recalledAt: m.recalledAt ?? ev.happenedAt }
              : m,
          )
        } else if ((ev.kind === 'member.joined' || ev.kind === 'member.left') && ev.roomId) {
          const snapshot = ev.rawSnapshot
          const name =
            typeof snapshot?.participant === 'object' && snapshot.participant
              ? String((snapshot.participant as { display_name?: unknown }).display_name ?? '参与者')
              : '参与者'
          const authorId =
            typeof snapshot?.participant === 'object' && snapshot.participant
              ? String((snapshot.participant as { id?: unknown }).id ?? 'system')
              : 'system'
          const roomId = ev.roomId
          const list = messagesByRoom[roomId] ?? []
          const sysId = `sys-${ev.eventId}`
          if (!list.some((m) => m.id === sysId)) {
            const sys: Message = {
              id: sysId,
              roomId,
              authorId,
              body: ev.kind === 'member.joined' ? `${name} 加入了会话` : `${name} 离开了会话`,
              sentAt: new Date(ev.happenedAt).toLocaleTimeString([], {
                hour: '2-digit',
                minute: '2-digit',
              }),
              day: '今天',
              delivery: 'delivered',
              system: true,
            }
            messagesByRoom[roomId] = [...list, sys]
            // 读 → 追 → 写必须在同一条链上（异步口不能再假定"读回来就是我刚放进去的"）。
            // 这里**不 await**：`set()` 的回调必须是同步的（zustand 的约定），
            // 而"系统消息落盘"不是渲染路径上的事 —— 写不进去只是这一条下次重来。
            void loadSysMessages(roomId, worldKey()).then((prior) =>
              saveSysMessages(roomId, worldKey(), [...prior, sys]),
            )
          }
        }
      }
      if (api && events.length > 0) {
        const roomsWithEvents = new Set(events.filter((ev) => ev.roomId).map((ev) => ev.roomId!))
        for (const roomId of roomsWithEvents) emitBroadcast({ type: 'messages.changed', roomId })
      }
      return { messagesByRoom }
    })
  }

  /**
   * **世界的身份**（`GET /v1/home`）—— 一次连接只问一次（memo，换连接时重置）。
   *
   * 问到了 ⇒ 记进 prefs，世界级事实自此按**世界**分键（`celira.world:<世界>:*`）；
   * **问不到 ⇒ 返回空串**，一切按凭据分键，行为与从前**一字不差**
   * （旧服务端、离线、演示通道、这一面还没实现 —— 都走这一档；**绝不因为这一问失败
   * 而让连接失败**）。
   */
  const ensureWorldIdentity = (a: WorldApi): Promise<string> => {
    if (worldIdentity) return worldIdentity
    worldIdentity = (async () => {
      if (typeof a.worldIdentity !== 'function') return ''
      try {
        const id = parseWorldIdentity(await a.worldIdentity())
        if (!id) return ''
        if (prefs.worldId !== id) {
          const token = prefs.token
          prefs = { ...prefs, worldId: id }
          // 写失败不抛（凭据与地址都不是事实，写不进去只是下次再问一遍）。
          void savePrefs(prefs)
          // **换世界 = 换一套用户侧偏好**（置顶/免打扰按世界分键）：重新读那一个世界的。
          // 不重读的话，切服务器之后旧世界的置顶会留在新世界的会话上。
          // 读失败按"没意见"（`loadRoomPrefs` 自己就宽容），绝不因为读偏好而让这一问失败。
          roomPrefs = await loadRoomPrefs(worldKeyOf(id, token))
          set({ worldId: id, rooms: applyRoomPrefs(get().rooms, roomPrefs) })
        }
        return id
      } catch {
        return ''
      }
    })()
    return worldIdentity
  }

  /**
   * **Agent 所有权（"哪些 Agent 是我的"）的取数**。
   *
   * 它是**世界侧事实**（住在这个 im 服务端里），所以"要先连上世界才知道"是事实顺序，
   * 不是耦合。但取数**不该钉在某一条成功路径上**——2026-09-25 拆分刀之前它只出现在
   * `connect()` 的 try 末尾，于是：IM 不通时走缓存回落那一支**不拉**、WS 重连的
   * `online` 回调**也不拉** ⇒ IM 抖一次之后 Ownership 清单就停在那儿，只能手动点重试。
   *
   * 现在由**「连接进入 connected」这一个状态**驱动（下面两处都在那一刻调它）。
   */
  const refreshOwnedOnConnected = () => {
    void get().refreshOwnedAgents()
  }

  const connect = async () => {
    // **演示通道拿到的是 mock 传输层**（2026-09-25 补回）：没有 token ⇒
    // `DemoSwitcher` / 深链接能进的那些页（chat / contacts / agents / profile）
    // 才有数据可渲染。没有它时 `store.rooms` 永远是 `[]`（只有这里会填），
    // 于是「只是想看页面」的人打开 `/app/chat` 只能看到「没有会话」。
    // 判据与真实/演示的分野**同一处**：手里有没有真实凭据（`prefs.isRealWorldSession`）。
    //
    // ⚠ **mock 要复用同一个实例，不能每次 `connect()` 都新建**：`connect()` 会被调用
    // 多次（`AppShell` 的 effect + `resolveFirstScreen`），而 store 各处都用
    // `owns = () => api === apiNow` 这道**所有权栅栏**丢弃"旧代"的在途结果。
    // 每次新建 ⇒ 前一次的请求结果全部作废，而那些**无条件设下的**加载标志
    // （`xxxLoading: true`）就再也没人落回 false ⇒ 页面**永久卡在加载态**
    // （2026-09-25 实测过一次：`/app/services` 10 秒后仍显示「正在读取服务列表…」；
    // 该页已于 2026-09-28 随服务层退役删除，**这条坑本身照旧成立**）。
    // 复用之后 `owns()` 恒真，在途结果能正常落地。
    // `CoreApi` 不在此列：它带 token / 服务器地址，换连接**必须**是新实例。
    const next: WorldApi = prefs.token
      ? (() => {
          const core = new CoreApi(prefs.token, prefs.serverUrl)
          core.onUpdate = syncFromApi
          return core
        })()
      : (api instanceof MockApi ? api : new MockApi())

    api = next
    // 只有 `CoreApi` 有连接状态回调（`MockApi` 没有连接可掉）—— 判据是**具体类型**，
    // 不再靠 `mode === 'core'`（那个判据恒真，演示通道也满足）。
    if (next instanceof CoreApi) {
      next.onConnectionChange = (state) => {
        if (state === 'online') {
          set({ connection: 'connected' })
          // ⚠ 事件 cursor 是在 `pollEvents` 里读的，而它的键是**世界级**的 ⇒ **先认世界再读**。
          // 这条 WS 的 `online` 可能早于 connect 自己的 `await`（core.ts 的 onopen），
          // 所以两条路都要过这一关；`ensureWorldIdentity` 是 memo 的，只会真问一次。
          void ensureWorldIdentity(next).then(() => {
            void pollEvents()
            void refreshRooms()
            // 重连也算"进入 connected" ⇒ Ownership 清单跟着回来（原先这里漏了，见上）。
            refreshOwnedOnConnected()
          })
        } else if (state === 'offline') {
          set({ connection: 'reconnecting' })
        }
      }
    }
    set({ connection: 'connecting', connectionError: undefined })
    try {
      const me = await next.me()
      // 认世界**在列房间与读 cursor 之前**：世界级事实的键要靠它（问不到就退回凭据分键）。
      await ensureWorldIdentity(next)
      const entries = await next.listRooms()
      registerLookup(entries, me)
      // **首屏的会话行预览**：`GET /v1/rooms` 本来就**不带**预览与时间（`core.ts` 如实给空串），
      // 而这一刻任何房间的历史都还没加载 ⇒ 直接 `set` 的话回访用户看到的是一屏「还没有消息」，
      // 要等他点开每一间房才逐个补上（2026-09-29 真机实测）。
      // 这里用**本机上一次已知的**那一份房间缓存把"还没有预览"的那些房间顶上（一次 IndexedDB
      // 读，不走网络）：它是客户端自己上一次的权威历史，属于**展示缓存**，与"连不上时拿房间缓存
      // 顶上"是同一类东西；权威历史一到（`loadRoomMessages` → `reprojectRoomPreviews`）就被覆盖。
      set({
        me,
        rooms: await seedPreviewsFromRoomsCache(mergeSelf(entries.map((e) => e.room), me), worldKey()),
        connection: 'connected',
      })
      await pollEvents()
      // ⚠ **唤醒要同时驱动"权威重读"**，不能只 `pollEvents()`。
      //
      // 真实路径：唤醒来自 WS，`pollEvents` 拉的 Human Event 里**带着新消息本身**
      // （`message.created` 的 snapshot），所以重读历史不是必需的；
      // 而演示通道的 mock **没有事件账本**（`listEvents` 如实返回空页，不编造事件）——
      // 只调 `pollEvents` 就等于什么都没发生，Agent 那句占位回复永远不出现在时间线里。
      //
      // `syncFromApi()`（= `onUpdate`）做的是"重读 rooms + 已进入房间的 messages"，
      // 那正是**权威重读**，与 WS 唤醒的收敛方式同形。两条都调：真实路径不受影响
      // （多一次权威重读，本来就是无事实唤醒该有的收敛），演示通道这才闭环。
      next.subscribeWakeup(() => {
        void pollEvents()
        syncFromApi()
      })
      // 通话那条流：**另一条 WS**（`/v1/call/stream`，带事实、不重放）。演示通道没有通话面
      // ⇒ `callApiOf` 回 `null`，一个字都不订阅（不许给 mock 编一套假的通话）。
      const callApi = callApiOf(next)
      callApi?.subscribeCallStream(
        (frame) => handleCallFrame(set, frame, get().me?.participant.id ?? ''),
        () => {
          // 流（重新）接上 ⇒ **重同步**：这一条不重放，错过就永远补不回来
          // （对方挂了、45 秒超时结束了，都只在 `call.state` 里出现过一次）。
          const current = get().call
          if (!current) return
          void callApi
            .getCall(current.call_id)
            .then(({ call }) => {
              if (call.state === 'ended') finishCall(set, call, get().me?.participant.id ?? '')
              else set({ call })
            })
            .catch(() => {
              // 读不到（典型：404 —— Core 重启过，进程内的 Call 全没了，契约 §2）
              // ⇒ 如实收尾，**不假装这通还在**。
              finishCall(set, { ...current, state: 'ended', ended_reason: null }, get().me?.participant.id ?? '')
            })
        },
      )
      void get().refreshContacts()
      // 黑名单与关系账同源、同样在建连时读一次：不读的话刷新页面之后「已拉黑」那一组是空的，
      // 而服务端明明还存着（"看不见也解除不了"是我们已经踩过一次的形状）。
      void get().refreshBlocks()
      // 连接进入 connected ⇒ Ownership 清单跟着拉（与 WS 那条 `online` 路同一处判据）。
      refreshOwnedOnConnected()
    } catch (e) {
      // ⚠ 原判据 `prefs.mode === 'core'` —— 恒真（`ConnectionMode` 只有一个取值）。
      // 去掉条件，行为一字未变：连不上时用上一次的房间缓存顶上，让界面还有东西可看。
      const cachedRooms = await loadRoomsCache(worldKey())
      if (cachedRooms && cachedRooms.length > 0) {
        registerLookup(
          cachedRooms.map((r) => ({ room: r, peer: r.peer ? [r.peer] : [] })),
          null,
        )
        set({
          connection: 'reconnecting',
          connectionError: undefined,
          rooms: cachedRooms,
        })
        next.subscribeWakeup(() => void pollEvents())
        return
      }
      set({
        connection: 'error',
        // ⚠ **上屏的必须是固定人话，不是 `e.message`**（2026-10-08）：壳的 Rust 侧连不上时
        // 抛的是 `error sending request for url (http://…)`（reqwest 的原文），浏览器抛的是
        // `TypeError: Failed to fetch` —— 两者都是**库的话**，而这一格直接渲染在登录页上
        // （`app/README.md` §5 第 15 行那条缺陷的现场）。原文只进控制台。
        connectionError: errorText(e),
      })
      console.error('Celira：连接失败（原文只在这里，界面显示的是固定人话）', e)
    }
  }

  /**
   * **用户侧偏好（置顶 / 免打扰）的翻转**：改本机存储 + 把新偏好投影到房间列表。
   *
   * 两条动作共用这一处（判据只有一份）；世界侧一个字都不发 —— 它们不是公共事实。
   *
   * **返回是否落盘**：内存里的偏好无论如何都生效（界面立刻响应），但存储写不进去时返回
   * `false` —— 用户下次开应用会发现置顶没了。**调用方不许把这个 `false` 吞掉**；
   * 目前 UI 侧还未消费它（如实登记的缺口：界面暂不提示"这次置顶不会被记住"）。
   *
   * ⚠ 2026-10-08 起存储那一半是**异步**的（走窄口）：内存先改（界面不等它），落盘结果如实回。
   */
  const toggleRoomPref = async (roomId: string, kind: 'pinned' | 'muted'): Promise<boolean> => {
    const next: RoomPrefs = { ...roomPrefs, [kind]: toggleId(roomPrefs[kind], roomId) }
    roomPrefs = next
    set({ rooms: applyRoomPrefs(get().rooms, next) })
    return saveRoomPrefs(worldKey(), next)
  }

  /**
   * **一条 pending 消息的两种收场**（`send` 与 `retry` 共用）。
   *
   * `settleSent`：把乐观那条换成服务端给的 id 并标 delivered —— 契约 §6.3 说重放
   * 返回的是**原** Message，所以 `retry` 打同一个 key 时这里拿到的是首次那条的 id，
   * 列表里不会出现第二条。
   * `settleFailed`：只改 delivery，**保留 id 与 idempotencyKey** —— 那颗重试按钮
   * 要靠这两格找到它、并复用同一把 key。
   */
  const settleSent = (roomId: string, localId: string, serverId: string) => {
    set((s) => ({
      messagesByRoom: {
        ...s.messagesByRoom,
        // ★ **去重式改名**（2026-10-16）：服务端回显可能**先**到了（合并那一页把它并了进来）
        //   ⇒ 直接改名会留下**同一 id 两条**，而虚拟列表的行身份就是消息 id
        //   ⇒ 两行画在同一个 `translateY` 上（真机上"两段字压在一行"就是这个）。
        [roomId]: settleOptimisticId(s.messagesByRoom[roomId] ?? [], localId, serverId),
      },
    }))
  }

  const settleFailed = (roomId: string, localId: string) => {
    set((s) => ({
      messagesByRoom: {
        ...s.messagesByRoom,
        [roomId]: (s.messagesByRoom[roomId] ?? []).map((m) =>
          m.id === localId ? { ...m, delivery: 'failed' as DeliveryState } : m,
        ),
      },
    }))
  }

  return {
    mode: prefs.mode,
    token: prefs.token,
    serverUrl: prefs.serverUrl,
    /** 当前连接的世界（`GET /v1/home` 的 `home_id`；空串 = 还不知道）。 */
    worldId: prefs.worldId,
    connection: 'idle',
    startupPending: true,
    needsLogin: false,
    me: null,
    rooms: [],
    messagesByRoom: {},
    messagesLoadErrorByRoom: {},
    nextBeforeByRoom: {},
    timelineLoadedByRoom: {},
    contacts: [],
    pendingContacts: [],
    blocks: [],
    ownedAgents: [],
    ownedAgentsLoading: false,
    ownedAgentsError: undefined,
    anjiaByAgent: {},
    membersByRoom: {},
  roomLiveByRoom: {},
    memberPresentationsByRoom: {},
    bindingsByAgent: {},
    credentialsByBinding: {},
    sessionsByRoom: {},
    trajectoriesByRoom: {},
    pausedByRoom: {},
    call: null,
    callRole: null,
    callMediaState: 'idle',
    callNotice: null,

    setStartupPending: (pending) => {
      set({ startupPending: pending })
    },

    setNeedsLogin: (needsLogin) => {
      set({ needsLogin })
    },

    setPrefs: (mode, token, serverUrl) => {
      // 仍在途的旧代请求自此全部失效（绝不能写进新模式/新 token 的 store 状态）；
      // 旧 api 立即失效。
      api = null
      // **换一份连接 = 换一个（还不知道的）世界**：世界身份清空、memo 重置 —— 下一次
      // 连接会重新问一遍 `GET /v1/home`（在问出来之前，世界级事实按凭据分键）。
      worldIdentity = null
      prefs = { mode, token, serverUrl: serverUrl ?? prefs.serverUrl, worldId: '' }
      void savePrefs(prefs)
      set({
        mode,
        token,
        serverUrl: prefs.serverUrl,
        worldId: '',
        connection: 'idle',
        startupPending: false,
        // 正门状态**不在这里改**：needsLogin 只由启动探测（`api/entry.ts` 的判定）
        // 与登录页自己的成功回调翻转。否则「保存一份连不上的凭据」会顺手把
        // 登录页收掉，用户就看不到失败原因了。
        me: null,
        rooms: [],
        messagesByRoom: {},
        messagesLoadErrorByRoom: {},
        nextBeforeByRoom: {},
        timelineLoadedByRoom: {},
        memberPresentationsByRoom: {},
        contacts: [],
        pendingContacts: [],
        blocks: [],
        ownedAgents: [],
        ownedAgentsLoading: false,
        ownedAgentsError: undefined,
        anjiaByAgent: {},
      })
      // ⚠ 原判据 `mode === 'core'` —— 恒真（`ConnectionMode` 只有一个取值）。
      // 这一句本来想说的是"换连接了，通知别的标签页"，与模式无关（审计 D18）。
      emitBroadcast({ type: 'prefs.changed' })
    },

    applyMyName: (displayName) => {
      set((s) =>
        s.me
          ? {
              me: {
                ...s.me,
                participant: { ...s.me.participant, display_name: displayName },
              },
            }
          : {},
      )
    },

    refreshRooms: async () => {
      await refreshRooms()
    },

    pollEvents: async () => {
      await pollEvents()
    },

    refreshContacts: async () => {
      const apiNow = api
      if (!apiNow?.listContacts) return
      try {
        const { accepted, pending } = await apiNow.listContacts()
        // 所有权：结果只能写回当前会话/当前 api。旧代迟到结果绝不覆盖新会话状态。
        if (api !== apiNow) return
        set({ contacts: accepted, pendingContacts: pending })
      } catch {
        // keep previous
      }
    },

    /**
     * 发一段关系申请，**把回执交回去**（页面拿它说"这条现在在等谁"）。
     *
     * 两条：
     * - 没有这条能力的连接上**抛错**，不静默返回 —— 从前它 `return` 一个 `undefined`，
     *   页面照样把按钮切成「请求已发送」，那是对着一件没发生的事报成功；
     * - 落定之后重读关系账（对端可能刚接受，或这条其实是对面先发过来的）：世界说了算，
     *   本地不猜。重读是 fire-and-forget，不挡住回执。
     */
    sendContactRequest: async (participantId, idempotencyKey) => {
      const apiNow = api
      if (!apiNow?.sendContactRequest) throw new Error('当前连接不提供发起联系人申请的能力')
      // idempotencyKey 由 core.ts 内部自生成（调用方传的被忽略）；
      // 授权/关系/幂等裁决全在 Core。
      const receipt = await apiNow.sendContactRequest(participantId, idempotencyKey)
      void get().refreshContacts()
      return receipt
    },

    acceptContact: async (humanId) => {
      if (!api?.acceptContact) return
      await api.acceptContact(humanId)
      void get().refreshContacts()
    },

    /**
     * 拒绝给我的那条申请。
     *
     * 与 `acceptContact` 一样，落定之后重读关系账：pending 那条从"等我回答"里消失，
     * 而**别的**读数的变化（比如对端名下有没有多出一段）由服务端说，不由本地猜。
     */
    declineContact: async (participantId) => {
      if (!api?.declineContact) throw new Error('当前连接不提供拒绝联系人申请的能力')
      await api.declineContact(participantId)
      void get().refreshContacts()
    },

    /**
     * 断交（单方面解除关系）。
     *
     * ⚠ **这是客户端第一个会"拿掉一间房"的动作**：契约规定段结束时双方在那个 human_direct
     * 房间的成员周期在同一事务内关闭 ⇒ 那间房从双方的会话列表里消失、永久封存。所以这里
     * 除了重读关系账，**必须同时重读房间列表**——否则左栏会留着一间点进去什么也读不到的房
     * （我们上次刚修过"缓存里留着旧房间"那一类问题，不在这条新路径上重犯）。
     */
    removeContact: async (participantId) => {
      if (!api?.removeContact) throw new Error('当前连接不提供解除关系的能力')
      await api.removeContact(participantId)
      await Promise.all([get().refreshContacts(), get().refreshRooms()])
    },

    /**
     * 拉黑。契约里 **拉黑 ⊇ 解除**：同一事务先结束既有 active 段 ⇒ 后果与 `removeContact`
     * 完全一样（房间同样被封存），所以这里做同样两件重读，再多读一次黑名单。
     */
    blockParticipant: async (participantId) => {
      if (!api?.blockParticipant) throw new Error('当前连接不提供拉黑的能力')
      await api.blockParticipant(participantId)
      await Promise.all([get().refreshContacts(), get().refreshRooms(), get().refreshBlocks()])
    },

    /** 解除拉黑。契约明写**不恢复关系** ⇒ 只重读黑名单与关系账，房间不会回来。 */
    unblockParticipant: async (participantId) => {
      if (!api?.unblockParticipant) throw new Error('当前连接不提供解除拉黑的能力')
      await api.unblockParticipant(participantId)
      await Promise.all([get().refreshBlocks(), get().refreshContacts()])
    },

    refreshBlocks: async () => {
      const apiNow = api
      if (!apiNow?.listBlocks) return
      try {
        const blocks = await apiNow.listBlocks()
        // 所有权栅栏：旧代迟到结果绝不覆盖新会话状态（与 `refreshContacts` 同款）。
        if (api !== apiNow) return
        set({ blocks })
      } catch {
        // keep previous
      }
    },

    createHumanDirectRoom: async (humanId) => {
      // ⚠ 2026-09-26：这里原先在能力缺失时 **`return 'r-guide'`** —— 把演示通道的
      // fixtures 房号当成"建好的房间"返回。调用方拿到它就去 `/app/chat/r-guide`，
      // 真实世界里那是一个**不存在的房间**：用户以为建成了，其实什么也没发生。
      // 与 B4 那四支同款改法：明确抛错，由调用方如实呈现（`errorText`）。
      if (!api?.createHumanDirectRoom) throw new Error('当前连接不提供建立 Human 直聊的能力')
      const { roomId } = await api.createHumanDirectRoom(humanId)
      return roomId
    },

    // 一间房 = 一段关系（Core 批次二）：同一对 Human–Agent 只有一间私人房——
    // 已经有就由 Core 回放原房（200 `idempotency_replayed`）。所以这里**就是**
    // "回到你们那间"那一步，没有"要不要新建"的分叉，也没有标题入参。
    createDirectRoom: async (agentId) => {
      if (!api?.createDirectRoom) throw new Error('当前连接不提供建立 Agent 直聊的能力')
      const { roomId } = await api.createDirectRoom(agentId)
      await refreshRooms()
      return roomId
    },

    createGroup: async (name, idempotencyKey) => {
      if (!api?.createGroup) return 'r-guide'
      const { roomId } = await api.createGroup(name, idempotencyKey)
      await refreshRooms()
      return roomId
    },

    createAgent: async (displayName, idempotencyKey) => {
      if (!api?.createAgent) return ''
      const { agentId } = await api.createAgent(displayName, idempotencyKey)
      if (agentId) {
        void get().refreshOwnedAgents()
        void get().refreshContacts()
        // **创建成功 ⇒ 自动把它接到这台机器上**（首刀步骤 2 的唯一正路）。
        // ⚠ 只在**有壳**时叫：浏览器里没有这条路径，那两句诚实标注（`CREATE_WONT_REPLY`）
        // 照旧是事实——所以这里连试都不试，不给浏览器留下一条"看着能做"的假路。
        if (hasAnjiaBridge()) void get().attachAgent(agentId)
      }
      return agentId
    },

    attachAgent: async (agentId) => {
      // 「还在跑」先落一格：页面据此显示进行中（这一态是**页面自己的**，不是壳回的）。
      const running: AnjiaResult = {
        agentId,
        kind: 'running',
        sub: null,
        pid: null,
        envRoot: null,
        logFile: null,
      }
      set((s) => ({ anjiaByAgent: { ...s.anjiaByAgent, [agentId]: running } }))
      // 壳那一问（**不吞错**：桥在、命令不在、壳内部出错，都会拿回一个词表里的态）。
      const result = await startAnjia(agentId)
      set((s) => ({ anjiaByAgent: { ...s.anjiaByAgent, [agentId]: result } }))
      return result
    },

    refreshOwnedAgents: async () => {
      const apiNow = api
      if (!apiNow?.listOwnedAgents) return
      const owns = () => api === apiNow
      set({ ownedAgentsLoading: true, ownedAgentsError: undefined })
      try {
        const agents = await apiNow.listOwnedAgents()
        // 所有权：结果只能写回当前会话/当前 api（旧代迟到结果绝不覆盖新会话状态）。
        if (!owns()) return
        set({ ownedAgents: agents, ownedAgentsLoading: false, ownedAgentsError: undefined })
      } catch (error) {
        if (!owns()) return
        set({ ownedAgentsLoading: false, ownedAgentsError: errorText(error) })
      }
    },

    refreshAgent: async (agentId) => {
      if (!api?.getAgent) return
      try {
        const res = await api.getAgent(agentId)
        set((s) => ({
          ownedAgents: s.ownedAgents.map((a) =>
            a.id === agentId
              ? {
                  ...a,
                  displayName: res.agent.display_name,
                  since: res.agent.created_at.slice(0, 10),
                }
              : a,
          ),
        }))
      } catch {
        // keep previous
      }
    },

    refreshRoom: async (roomId) => {
      if (!api?.getRoom) return
      try {
        const res = await api.getRoom(roomId)
        set((s) => {
          const rooms = s.rooms.map((r) => {
            if (r.id !== roomId) return r
            const peer = res.peer
              ? toParticipant(res.peer)
              : undefined
            return {
              ...r,
              name: res.room.name ?? peer?.displayName ?? r.name,
              kind: (res.room.kind as Room['kind']) ?? r.kind,
              peer,
              participants: peer ? [peer] : r.participants,
              participantIds: peer ? [peer.id] : r.participantIds,
              unreadCount: res.read_state.unread_count,
              // 「对方读没读我最后那条」（契约 CORE_V1_READ_RECEIPT.md §2）：
              // **原样搬**——缺字段 / null 都是"这一格不出现"，不许就近映射成 'unread'。
              peerRead: res.peer_read ?? undefined,
            }
          })
          // 简介单独走纯函数（`api/roomProfile.ts`）：只改目标房，
          // **其它房间已读到的 profile 原样保留** —— 列表端点本来就不带这一格。
          return { rooms: applyRoomProfile(rooms, roomId, res.profile) }
        })
      } catch {
        // keep previous
      }
    },

    updateRoomProfile: async (roomId, input) => {
      if (!api?.putRoomProfile) {
        throw new Error('当前连接不支持编辑群简介')
      }
      // 失败**原样抛**（带 Core 稳定分类码）：UI 据此给人话、判 409 后重读；
      // 绝不吞成"保存成功"。
      await api.putRoomProfile(roomId, input)
      // 写后权威重读：版本只认 Core 读面，本地绝不 +1。
      await get().refreshRoom(roomId)
    },

    refreshBindings: async (agentId) => {
      if (!api?.listBackendBindings) return
      try {
        const bindings = await api.listBackendBindings(agentId)
        set((s) => ({ bindingsByAgent: { ...s.bindingsByAgent, [agentId]: bindings } }))
        const active = bindings.find((b) => b.status === 'active')
        if (active) void get().refreshCredentials(agentId, active.id)
      } catch {
        // keep previous
      }
    },

    createBackendBinding: async (agentId, label) => {
      // ⚠ 2026-09-26：这里原先是静默 `return` —— 于是演示通道里那个按钮**点了什么也不发生、
      // 也不报错**（页面当时只探到读面就渲染了按钮）。改成与 `revokeAgent` **同款**的
      // 明确抛错：`AGENTS.md` 的纪律是「**不用空集合、默认成功、统一字符串或宽泛兜底
      // 吞掉身份拒绝、读写失败及未知结果**」，静默 return 正是"默认成功"吞掉失败。
      if (!api?.createBackendBinding) throw new Error('当前连接不提供创建接入的能力')
      await api.createBackendBinding(agentId, label)
      await get().refreshBindings(agentId)
    },

    revokeBackendBinding: async (agentId, bindingId) => {
      if (!api?.revokeBackendBinding) throw new Error('当前连接不提供撤销接入的能力')
      await api.revokeBackendBinding(agentId, bindingId)
      await get().refreshBindings(agentId)
    },

    refreshCredentials: async (agentId, bindingId) => {
      if (!api?.listCredentials) return
      try {
        const credentials = await api.listCredentials(agentId, bindingId)
        set((s) => ({
          // 键走唯一权威 `credentialScopeKey`（消费侧同一个 import）：两侧各写一遍模板串
          // 曾在消费侧静默退化成"没有凭据"这个假事实（见 `api/credentials.ts` 模块头）。
          credentialsByBinding: {
            ...s.credentialsByBinding,
            [credentialScopeKey(agentId, bindingId)]: credentials,
          },
        }))
      } catch {
        // keep previous
      }
    },

    issueCredential: async (agentId, bindingId, replacesCredentialId) => {
      if (!api?.issueCredential) throw new Error('当前连接不提供签发凭据的能力')
      const { token } = await api.issueCredential(agentId, bindingId, replacesCredentialId)
      await get().refreshCredentials(agentId, bindingId)
      return token
    },

    revokeCredential: async (agentId, bindingId, credentialId) => {
      if (!api?.revokeCredential) throw new Error('当前连接不提供撤销凭据的能力')
      await api.revokeCredential(agentId, bindingId, credentialId)
      await get().refreshCredentials(agentId, bindingId)
    },

    refreshRoomLive: async (roomId) => {
      if (!api?.roomLive) return
      try {
        const snapshot = await api.roomLive(roomId)
        set((s) => ({ roomLiveByRoom: { ...s.roomLiveByRoom, [roomId]: snapshot } }))
      } catch {
        // 读不到就保留上一次：进行态是易失投影，**不**据此推断"它离线了"。
      }
    },

    notifyTyping: async (roomId) => {
      // best-effort：core 实现里已经吞掉失败，这里再兜一层，绝不打扰用户。
      try {
        await api?.notifyTyping?.(roomId)
      } catch {
        // 见上。
      }
    },

    refreshRoomMembers: async (roomId) => {
      if (!api?.listRoomMembers) return
      try {
        const members = await api.listRoomMembers(roomId)
        const participants = members.map((m) => toParticipant(m.participant))
        set((s) => ({
          membersByRoom: { ...s.membersByRoom, [roomId]: members },
          rooms: s.rooms.map((room) =>
            room.id === roomId
              ? {
                  ...room,
                  participants,
                  participantIds: participants.map((participant) => participant.id),
                }
              : room,
          ),
        }))
      } catch {
        // keep previous
      }
    },

    refreshMemberPresentations: async (roomId) => {
      if (!api?.getMemberPresentations) return
      try {
        const res = await api.getMemberPresentations(roomId)
        set((s) => ({
          memberPresentationsByRoom: { ...s.memberPresentationsByRoom, [roomId]: res.members },
        }))
      } catch {
        // keep previous
      }
    },

    getRoomPresentation: async (roomId, participantId) => {
      if (!api?.getRoomPresentation) {
        throw new Error('当前连接不支持读取成员介绍')
      }
      return api.getRoomPresentation(roomId, participantId)
    },

    updateRoomPresentation: async (roomId, participantId, input, idempotencyKey) => {
      if (!api?.putRoomPresentation) return
      if (api?.getMemberPresentations) {
        try {
          await api.putRoomPresentation(roomId, participantId, { ...input, idempotencyKey })
        } catch (e) {
          // 稳定文案 + 携带稳定分类码（供 UI 判 409 stale 后触发权威重读最新
          // 版本；绝不把原始错误/ key 漏出）。
          const message = roomPresentationErrorMessage(e)
          const err = new Error(message)
          const code = e && typeof e === 'object' && 'code' in e ? String(e.code) : ''
          if (code) (err as { code?: string }).code = code
          throw err
        }
        await get().refreshMemberPresentations(roomId)
        return
      }
      throw new Error('当前连接不支持编辑房间成员介绍')
    },

    inviteMember: async (roomId, participantId, idempotencyKey) => {
      if (!api?.inviteMember) return
      await api.inviteMember(roomId, participantId, idempotencyKey)
      await get().refreshRoomMembers(roomId)
      void refreshRooms()
    },

    removeMember: async (roomId, participantId) => {
      if (!api?.removeMember) return
      await api.removeMember(roomId, participantId)
      await get().refreshRoomMembers(roomId)
      void refreshRooms()
    },

    leaveRoom: async (roomId) => {
      if (!api?.leaveRoom) return
      await api.leaveRoom(roomId)
      void clearRoomCache(roomId)
      set((s) => {
        const messagesByRoom = { ...s.messagesByRoom }
        const rooms = s.rooms.filter((r) => r.id !== roomId)
        delete messagesByRoom[roomId]
        return {
      rooms,
      messagesByRoom,
      membersByRoom: omitKey(s.membersByRoom, roomId),
      roomLiveByRoom: omitKey(s.roomLiveByRoom, roomId),
    }
      })
    },

    transferOwnership: async (roomId, participantId, idempotencyKey) => {
      if (!api?.transferOwnership) return
      await api.transferOwnership(roomId, participantId, idempotencyKey)
      await get().refreshRoomMembers(roomId)
    },

    revokeAgent: async (agentId) => {
      if (!api?.revokeAgent) {
        throw new Error('当前连接不提供 Agent 撤销能力')
      }
      await api.revokeAgent(agentId)
      await get().refreshOwnedAgents()
    },

    revokeSession: async () => {
      if (!api?.revokeSession) return
      await api.revokeSession()
      get().disconnect()
      // 退出登录只清凭据与模式；**服务器地址保留**（用户下次还要连同一台）。
      // 归一到 core：Host 已退役，重置后的形状 = 「没连过的直连」。
      // ⚠ 三族「世界级」键的键名里可能**嵌着 token**（世界身份拿不到时退回凭据，
      // 见 `worldKey`）⇒ 必须按**退出前那个世界的键**把三族一起清掉
      // （`worldKey()` 在 prefs 重置之后就取不到了）。2026-09-26 修：原先只清 cursor 一个。
      const keyAtLogout = worldKey()
      prefs = { mode: 'core', token: '', serverUrl: prefs.serverUrl, worldId: '' }
      worldIdentity = null
      void savePrefs(prefs)
      // 清整族是**异步的**（走窄口）⇒ 不挡着登出这一下（凭据已经清了，清缓存可以慢一步）。
      void clearWorldScopedKeys(keyAtLogout)
      set({ mode: prefs.mode, token: prefs.token, serverUrl: prefs.serverUrl, worldId: '' })
    },

    connect,

    disconnect: () => {
      // 断开使仍在途的旧代请求失效，不许其写回新状态。
      api = null
      // 补拉的退避重试也跟着停（登出/断开之后没人该再去拉）。
      clearPollRetry()
      // 世界身份那一次问话也跟着作废（下一次连接重新问）。
      worldIdentity = null
      // ⚠ **通话要连媒体一起收**：断开/登出之后还握着麦克风是最不该留的东西
      //   （状态清掉而轨道还开着，用户那边的麦克风灯会一直亮）。
      activeMedia?.close()
      activeMedia = null
      set({
        connection: 'idle',
        startupPending: false,
        me: null,
        contacts: [],
        pendingContacts: [],
        blocks: [],
        ownedAgents: [],
        ownedAgentsLoading: false,
        ownedAgentsError: undefined,
        anjiaByAgent: {},
        call: null,
        callRole: null,
        callMediaState: 'idle',
        callNotice: null,
      })
    },

    retryConnection: async () => {
      await get().connect()
    },

    loadRoomMessages: async (roomId) => {
      if (!api) return
      // 每次尝试开始先清掉这个房间上一次的失败读数（否则重试成功后仍挂着旧错误）。
      set((s) => {
        if (!(roomId in s.messagesLoadErrorByRoom)) return {}
        const next = { ...s.messagesLoadErrorByRoom }
        delete next[roomId]
        return { messagesLoadErrorByRoom: next }
      })
      // ⚠ 这三处原判据都是 `api.mode === 'core'` —— 恒真（`ConnectionMode` 只有一个取值）。
      // 去掉条件，行为一字未变：先拿缓存顶上（让界面立刻有东西），再去拉权威历史。
      const cached = await loadRoomCache(roomId, worldKey())
      if (cached && cached.messages.length > 0) {
        const sys = await loadSysMessages(roomId, worldKey())
        // ★ **合并**（同 `refreshMessages` 那条判据，2026-10-16）：这一间**本次会话里已经翻过
        //   历史**时，缓存这一页只有 50 条 ⇒ 替换就等于把翻出来的抹掉 ✗。
        const seeded = [...sys, ...cached.messages]
        trackNewMessages(roomId, seeded)
        set((s) => ({
          messagesByRoom: {
            ...s.messagesByRoom,
            [roomId]: applyRoomPage({
              existing: s.messagesByRoom[roomId] ?? [],
              page: seeded,
              loaded: s.timelineLoadedByRoom[roomId] === true,
            }),
          },
          timelineLoadedByRoom: { ...s.timelineLoadedByRoom, [roomId]: true },
          // 同上：已经有"更早的游标"就别用缓存那一份顶掉它。
          nextBeforeByRoom:
            s.nextBeforeByRoom[roomId] === undefined || s.nextBeforeByRoom[roomId] === null
              ? { ...s.nextBeforeByRoom, [roomId]: cached.nextBefore }
              : s.nextBeforeByRoom,
        }))
        // 缓存顶上来的那一刻，会话行就该跟着有预览（不等权威历史回来）。
        reprojectRoomPreviews()
      }
      try {
        const { messages, nextBefore } = await api.listMessages(roomId)
        const sys = await loadSysMessages(roomId, worldKey())
        const latest = [...sys, ...messages]
        trackNewMessages(roomId, latest)
        scheduleCacheWrite(roomId, worldKey(), messages, nextBefore)
        set((s) => ({
          // ★ **合并**：权威历史到了也只是"最新一页"，翻过的历史照旧留在前头（同上）。
          // ⚠ 2026-10-20：同上，先看窗口读过没有 —— 只有片段时以这一页为准。
          messagesByRoom: {
            ...s.messagesByRoom,
            [roomId]: applyRoomPage({
              existing: s.messagesByRoom[roomId] ?? [],
              page: latest,
              loaded: s.timelineLoadedByRoom[roomId] === true,
            }),
          },
          timelineLoadedByRoom: { ...s.timelineLoadedByRoom, [roomId]: true },
          nextBeforeByRoom:
            s.nextBeforeByRoom[roomId] === undefined || s.nextBeforeByRoom[roomId] === null
              ? { ...s.nextBeforeByRoom, [roomId]: nextBefore }
              : s.nextBeforeByRoom,
        }))
        // 历史到手的这一刻补投影：否则左边那一行会一直说「还没有消息」（见上面那段注释）。
        reprojectRoomPreviews()
      } catch (error) {
        // 读失败**不许静默**（旧实现是 `keep previous`）：这是「这个会话还没有消息」那句
        // 假话的来源 —— 首屏读不到时那句话会一直挂着，看起来像历史被清空了。
        set((s) => ({
          messagesLoadErrorByRoom: { ...s.messagesLoadErrorByRoom, [roomId]: errorText(error) },
        }))
      }
    },

    loadOlder: async (roomId) => {
      if (!api) return 0
      const before = get().nextBeforeByRoom[roomId]
      if (!before) return 0
      // ⚠⚠ **必须"连着对象"调用**（2026-10-16 真机 + 探针一起抓到的真 bug）：
      //   从前这里写成 `const loadOlderFn = api.loadOlder` 再 `loadOlderFn(...)` ——
      //   方法被**摘下来**之后 `this` 就丢了，`core.ts` 里那句 `this.api(...)` 抛
      //   `TypeError: Cannot read properties of undefined (reading 'api')`，
      //   而下面的 `catch` 又把它**静默吞掉**（`return 0`）⇒ "加载更早"**从来没成功过一次**，
      //   界面上只是"点了没反应/滚到顶没动静"。⇒ 判据两条：**方法不许摘下来调用**、
      //   **翻页失败不许静默**（下面改成抛出，由页面说出来并给重试）。
      const loadOlderFn = (
        api as {
          loadOlder?: (r: string, b: string) => Promise<{ messages: Message[]; nextBefore: string | null }>
        }
      ).loadOlder
      if (!loadOlderFn) return 0
      // 每房间 in-flight 守卫：重复/并发翻页只放行一次；cursor 是 opaque 值，
      // 只按 nextBeforeByRoom 曾存储的值原样透传，绝不解析、绝不自行构造。
      if (!olderLoadGuard.enter(roomId)) return 0
      try {
        // 连着对象调用（`loadOlderFn` 只用来判"这一档在不在"）⇒ `this` 不会丢。
        const { messages, nextBefore } = await api.loadOlder!(roomId, before)
        // 翻回来的是**历史**：登记为"见过"（不发通知），免得后续 diff 把旧的当新的播一遍。
        markMessagesSeen(messages)
        set((s) => ({
          messagesByRoom: {
            ...s.messagesByRoom,
            [roomId]: mergeOlderPage(s.messagesByRoom[roomId] ?? [], messages),
          },
          nextBeforeByRoom: { ...s.nextBeforeByRoom, [roomId]: nextBefore },
        }))
        return messages.length
      } catch (error) {
        // **不许静默**：翻页失败要说出去（页面在顶部给一行"读更早失败 + 重试"）。
        // 游标保持不动 ⇒ 重试就是同一页，不会跳过任何历史。
        throw error
      } finally {
        olderLoadGuard.exit(roomId)
      }
    },

    send: async (roomId, input) => {
      if (!api) return
      const authorId = get()?.me?.participant?.id ?? 'self'
      // **每个逻辑发送一支新 key**，并把它留在 pending 那条消息上 —— 失败重试要复用它
      // （契约 §6.3：同 key 同 payload ⇒ 重放原 Message，不会多出第二条公共消息）。
      const idempotencyKey = newIdempotencyKey()
      const optimistic: Message = {
        id: `local-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`,
        roomId,
        authorId,
        body: input.body ?? '',
        sentAt: '刚刚',
        day: '今天',
        delivery: 'sending',
        replyToId: input.replyToId,
        mentions: input.mentions,
        attachments: input.attachments,
        idempotencyKey,
      }
      set((s) => ({
        messagesByRoom: { ...s.messagesByRoom, [roomId]: [...(s.messagesByRoom[roomId] ?? []), optimistic] },
      }))
      try {
        const { messageId } = await api.sendMessage(roomId, { ...input, authorId, idempotencyKey })
        settleSent(roomId, optimistic.id, messageId)
        // ⚠ 原判据 `api?.mode === 'core'` —— 恒真（`ConnectionMode` 只有一个取值），
        // 它实际起的作用是空值守卫。改写成它真正在说的那句话（审计 D18）。
        if (api) void pollEvents()
      } catch {
        settleFailed(roomId, optimistic.id)
      }
    },

    retry: async (messageId) => {
      if (!api) return
      // 找到那条失败的 pending：它身上留着首次那把 Idempotency-Key。
      let target: Message | undefined
      for (const list of Object.values(get().messagesByRoom)) {
        const hit = list.find((m) => m.id === messageId)
        if (hit) {
          target = hit
          break
        }
      }
      if (!target) return
      const { roomId, idempotencyKey } = target
      const payload = {
        body: target.body,
        replyToId: target.replyToId,
        attachments: target.attachments,
        mentions: target.mentions,
      }
      set((s) => ({
        messagesByRoom: {
          ...s.messagesByRoom,
          [roomId]: (s.messagesByRoom[roomId] ?? []).map((m) =>
            m.id === messageId ? { ...m, delivery: 'sending' as DeliveryState } : m,
          ),
        },
      }))
      try {
        // ⚠ 复用同一把 key（拿不到时 `core.ts` 会新生成一支 —— 那是退化路径，
        // 只在"这条 pending 不是本机造出来的"时才会发生）。
        const { messageId: newId } = await api.sendMessage(roomId, {
          ...payload,
          authorId: target.authorId,
          idempotencyKey,
        })
        settleSent(roomId, messageId, newId)
        if (api) void pollEvents()
      } catch {
        settleFailed(roomId, messageId)
      }
    },

    recall: async (roomId, messageId) => {
      if (!api) return
      await api.recallMessage(roomId, messageId)
      void pollEvents()
    },

    markRoomRead: async (roomId) => {
      if (api?.markRoomRead) {
        try {
          await api.markRoomRead(roomId)
        } catch {
          // offline: 本地先清未读，恢复后由事件循环纠正
        }
      }
      set((s) => ({
        rooms: s.rooms.map((r) => (r.id === roomId ? { ...r, unreadCount: 0 } : r)),
      }))
      if (api) emitBroadcast({ type: 'read.cursor', roomId })
    },

    togglePinned: async (roomId) => {
      await toggleRoomPref(roomId, 'pinned')
    },

    toggleMuted: async (roomId) => {
      await toggleRoomPref(roomId, 'muted')
    },

    setRoomAttention: async (roomId, level) => {
      const apiNow = getApi()
      // 这个世界没有这一档（未来某个实现没接）：不假装写成功 —— 但也不制造一个
      // "拨了没反应"的开关（界面按能力探测决定画不画它，见 `RoomInfoDrawer`）。
      if (!apiNow?.setRoomAttention) return
      const current = get().rooms.find((r) => r.id === roomId)?.attention ?? DEFAULT_ROOM_ATTENTION
      if (current === level) return
      // ① 乐观：先改列表（开关立刻到新位置；门控也立刻按新档判）。
      set((s) => ({ rooms: withRoomAttention(s.rooms, roomId, level) }))
      try {
        // ② 世界侧权威：响应回的是**写后的取值**，以它为准落回（通常就是 `level`）。
        const applied = await apiNow.setRoomAttention(roomId, level)
        set((s) => ({ rooms: withRoomAttention(s.rooms, roomId, applied) }))
      } catch (error) {
        // ③ 失败**回退 + 重抛**：静默吞掉正是这一格以前的病（拨了没反应、自己弹回去）。
        //    那句话由调用方说（`RoomInfoDrawer` 的 `runAction` → `roomActionErrorText`）。
        set((s) => ({ rooms: withRoomAttention(s.rooms, roomId, current) }))
        throw error
      }
    },

    // ── 通话（`CORE_V1_CALL.md`，Call P0 窄解冻切片）──────────────────────
    //
    // 这一组是**唯一**允许碰信令与媒体面的地方（页面只调它们）。四条纪律：
    // 1. **判据只有一处**：能不能打、角色是谁，都由 `callBlockedReason` / `caller_id` 判；
    // 2. **状态先落内存再发请求**（与 `toggleRoomPref` 同形）：UI 立刻有反应，
    //    请求失败时把话说出来并回退 —— 不许让按钮"按了没反应"；
    // 3. **媒体面与状态机分开**：媒体对象在模块级 `activeMedia` 里（它不是 React 状态），
    //    store 只记"媒体现在什么态"（`callMediaState`）；
    // 4. **`ended` 之后清 `call`、留 `callNotice`**：那一通不再是"当前这一通"，
    //    但"对方拒接了"这句话得留在屏幕上。

    startCall: async (roomId, calleeId) => {
      const api = getApi()
      const callApi = callApiOf(api)
      const state = get()
      const room = state.rooms.find((r) => r.id === roomId)
      const peerId = calleeId || room?.peer?.id
      if (!callApi) {
        set({ callNotice: '这个世界没有通话能力' })
        return
      }
      const blocked = callBlockedReason({
        selfId: state.me?.participant.id ?? '',
        roomKind: room?.kind,
        peerId,
        activeCall: state.call,
      })
      if (blocked) {
        set({ callNotice: blocked })
        return
      }
      set({ callNotice: null })
      try {
        const { call, created } = await callApi.initiateCall(roomId, peerId as string, 'audio')
        // ⚠ `created === false` = **复用**了既有 active Call（契约 §5）：那不是"我又打了一通"，
        //   所以不要重置媒体面（重来一遍会白白再采集一次麦克风）。
        set({ call, callRole: call.caller_id === state.me?.participant.id ? 'caller' : 'callee' })
        if (!created) return
        activeMedia?.close()
        activeMedia = createCallMedia({
          role: 'caller',
          send: (kind, payload) => void sendCallSignal(call.call_id, kind, payload),
          onState: (mediaState) => set({ callMediaState: mediaState }),
          onFailure: (failure, detail) => {
            if (detail) console.warn('Celira：通话媒体面失败（原文只在这里）', failure, detail)
            set({ callNotice: CALL_MEDIA_FAILURE_TEXT[failure] })
          },
        })
        await activeMedia.start()
      } catch (error) {
        // ⚠ 上屏的是**固定人话**（`contractErrors.ts` 的 call 那五个码），不是库的原文。
        set({ callNotice: errorText(error) })
      }
    },

    acceptCall: async () => {
      const callApi = callApiOf(getApi())
      const current = get().call
      if (!callApi || !current || current.state === 'ended') return
      set({ callNotice: null })
      try {
        const { call } = await callApi.acceptCall(current.call_id)
        set({ call })
        // 接听这一下才真正采集麦克风（振铃期间只收信令 —— 见 `callMedia.ts` 文件头）。
        if (!activeMedia) {
          activeMedia = createCallMedia({
            role: 'callee',
            send: (kind, payload) => void sendCallSignal(call.call_id, kind, payload),
            onState: (mediaState) => set({ callMediaState: mediaState }),
            onFailure: (failure, detail) => {
              if (detail) console.warn('Celira：通话媒体面失败（原文只在这里）', failure, detail)
              set({ callNotice: CALL_MEDIA_FAILURE_TEXT[failure] })
            },
          })
        }
        await activeMedia.accept()
      } catch (error) {
        set({ callNotice: errorText(error) })
      }
    },

    declineCall: async () => {
      const callApi = callApiOf(getApi())
      const current = get().call
      if (!callApi || !current) return
      try {
        const { call } = await callApi.declineCall(current.call_id)
        finishCall(set, call, get().me?.participant.id ?? '')
      } catch (error) {
        set({ callNotice: errorText(error) })
      }
    },

    hangupCall: async () => {
      const callApi = callApiOf(getApi())
      const current = get().call
      if (!callApi || !current) return
      try {
        const { call } = await callApi.hangupCall(current.call_id)
        finishCall(set, call, get().me?.participant.id ?? '')
      } catch (error) {
        set({ callNotice: errorText(error) })
      }
    },

    clearCallNotice: () => {
      set({ callNotice: null })
    },
  }
})

// ── fixtures 回落的总闸（TASK_56）───────────────────────────────────────────
//
// `fixtures/data.ts` 的目录层、`selfIdentity.ts` 的 SELF_ID 兜底，都从这里问
// 「这一轮允许回落假数据吗」。**注册一次、读当前会话**，所以不存在"某处改会话
// 忘了同步"的漏点（判据本体只有一份：`prefs.isRealWorldSession`）。
//
// 为什么闸门在这里而不在 fixtures 模块里：fixtures 是**纯数据**，不该知道会话
// 状态；会话状态的唯一权威是 store。接线放这里，落点也就只有这一处。
registerFixtureFallbackGate(
  () => !isRealWorldSession(useAppStore.getState()),
)

let suppressBroadcast = false

if (typeof window !== 'undefined') {
  subscribeBroadcast((msg) => {
    const s = useAppStore.getState()
    // ⚠ 原判据 `s.mode !== 'core'` —— 恒假（`ConnectionMode` 只有一个取值），
    // 整行是死代码。已删（2026-09-26，审计 D18）。
    const run = async (fn: () => Promise<void> | void) => {
      suppressBroadcast = true
      try {
        await fn()
      } finally {
        suppressBroadcast = false
      }
    }
    switch (msg.type) {
      case 'rooms.changed':
        void run(() => s.refreshRooms())
        break
      case 'messages.changed':
        void run(async () => {
          await s.pollEvents()
          await s.refreshRooms()
        })
        break
      case 'read.cursor':
        void run(() => s.refreshRooms())
        break
      case 'prefs.changed':
        void run(async () => {
          prefs = await loadPrefs()
          // 另一个标签页换了连接 ⇒ 世界身份那一次问话作废，重新问。
          worldIdentity = null
          useAppStore.setState({
            mode: prefs.mode,
            token: prefs.token,
            serverUrl: prefs.serverUrl,
            worldId: prefs.worldId,
          })
          // ⚠ 原判据 `prefs.mode === 'core' && prefs.token` —— 前半恒真；
          // 这句话真正在说的是"**有凭据才自动连**"（审计 D18）。
          if (prefs.token) await s.connect()
        })
        break
    }
  })
}

const EMPTY_MESSAGES: Message[] = []

export function useRooms(): Room[] {
  return useAppStore((s) => s.rooms)
}

export function useRoomMessages(roomId: string): Message[] {
  return useAppStore((s) => s.messagesByRoom[roomId] ?? EMPTY_MESSAGES)
}

export function useConnection() {
  return useAppStore(
    useShallow((s) => ({
      mode: s.mode,
      token: s.token,
      serverUrl: s.serverUrl,
      connection: s.connection,
      connectionError: s.connectionError,
      startupPending: s.startupPending,
      needsLogin: s.needsLogin,
      me: s.me,
    })),
  )
}

