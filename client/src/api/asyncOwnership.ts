// 异步结果的所有权栅栏与时间线合并（**与宿主无关**；TASK_64 从 `hostMessages.ts` 搬出来）。
//
// 为什么搬：这三组东西原先住在 `hostMessages.ts`（Host 消息数据面）里，但 core 的
// 真路径一直在用它们——`store.ts` 的每房翻页守卫与前插去重、`MessageInput.tsx` 的
// 附件上传批次所有权。Host 全托管路径退役、该文件整份删除时，**直接删会把 core 的
// 消费者一起带走**，所以先搬（与 §10.1 那三件中性 helper 同一处置）。
//
// 搬时**就地改名**：`HostAttachmentUploadFence` 一族去掉 host 前缀——通用词带 host
// 名字就是撒谎，一词一主。逻辑与文档注释一字未改。
//
// 本模块零 React / 零 DOM / 零持久化依赖，可在 `node --test` 下直接导入测试。

import type { DeliveryState, Message } from './domain'

/** 每房间 in-flight 守卫：StrictMode/路由 Effect/并发的重复请求只放行一个。 */
export interface InFlightGuard {
  /** 尝试独占 key；已被同 key 占用（进行中）返回 false。 */
  enter(key: string): boolean
  exit(key: string): void
  isInFlight(key: string): boolean
}

export function createInFlightGuard(): InFlightGuard {
  const active = new Set<string>()
  return {
    enter: (key) => {
      if (active.has(key)) return false
      active.add(key)
      return true
    },
    exit: (key) => {
      active.delete(key)
    },
    isInFlight: (key) => active.has(key),
  }
}

/**
 * 附件上传批次的所有权栅栏。
 *
 * 附件上传由 UI 组件发起，但结果要跨越任意数量的 await 才能回到组件。
 * 因此不能只依赖 React state（旧闭包可能仍持有旧房间），也不能只比较房间
 * 字符串（同一房间重建 api 后旧结果仍可能迟到）。每个批次同时钉住
 * roomId、精确 api 实例和 generation；开始新批次或生命周期边界时推进代并
 * 替换 active owner。旧批次在 success、failure、finally 的每个提交点都必须
 * 通过 `isCurrent`，否则一律 inert。这里不触碰服务端 pending：上传成功后未
 * 绑定的附件仍由 Core 的维护任务负责清理。
 */
export interface AttachmentUploadBatch<TApi = unknown> {
  readonly roomId: string
  readonly api: TApi
  readonly generation: number
}

export class AttachmentUploadFence<TApi = unknown> {
  private generation = 0
  private active: AttachmentUploadBatch<TApi> | undefined

  /** 开始新批次；新批次立即使旧批次失效，即使旧批次尚未 settle。 */
  begin(roomId: string, api: TApi): AttachmentUploadBatch<TApi> {
    this.generation += 1
    const batch: AttachmentUploadBatch<TApi> = {
      roomId,
      api,
      generation: this.generation,
    }
    this.active = batch
    return batch
  }

  /** 当前批次 token；仅用于宿主在开始新批次前清理该批次临时资源。 */
  current(): AttachmentUploadBatch<TApi> | undefined {
    return this.active
  }

  /** 生命周期/房间/API 会话边界：使当前批次及其所有迟到结果失效。 */
  invalidate(): void {
    this.generation += 1
    this.active = undefined
  }

  /** success/failure/finally 提交前的全部所有权条件。 */
  isCurrent(batch: AttachmentUploadBatch<TApi>, roomId: string, api: TApi): boolean {
    return (
      this.generation === batch.generation &&
      this.active === batch &&
      batch.roomId === roomId &&
      batch.api === api
    )
  }

  /** 只结束仍是 active 的批次；旧 finally 不能释放新批次。 */
  finish(batch: AttachmentUploadBatch<TApi>): boolean {
    if (this.generation !== batch.generation || this.active !== batch) return false
    this.active = undefined
    return true
  }
}

export function createAttachmentUploadFence<TApi = unknown>(): AttachmentUploadFence<TApi> {
  return new AttachmentUploadFence<TApi>()
}

/** 按 Core message id 去重，把更旧的一页原样前插到已有时间线前，
 * 不解析 cursor、不自行排序（Core 时间线顺序不在浏览器复制）。 */
export function mergeOlderPage(existing: Message[], olderPage: Message[]): Message[] {
  const result: Message[] = []
  const seen = new Set<string>()
  for (const m of olderPage) {
    if (seen.has(m.id)) continue
    seen.add(m.id)
    result.push(m)
  }
  for (const m of existing) {
    if (seen.has(m.id)) continue
    seen.add(m.id)
    result.push(m)
  }
  return result
}

/**
 * ★ **把服务端「最新一页」并进已有时间线**（2026-10-16 主人真机反馈："加载更早消息…机制有问题？"）。
 *
 * ## 为什么不能再"整条替换"（这一条就是那个机制问题）
 *
 * 从前 `refreshMessages`（每次 WS 唤醒 = **每来一条新消息**）这样写：
 * `messagesByRoom[roomId] = [...sys, ...最新一页]` —— **服务端那一页只有 50 条**，
 * 于是：人往上翻、`loadOlder` 把更早的几百条并了进来（那一半是对的），
 * **下一条新消息一到，翻出来的历史被整条抹掉** ✗ ⇒ 视图跳回去、"加载更早"等于白加载；
 * 同一个 `set` 还把**更早的游标**（`nextBeforeByRoom`）覆盖成最新一页的游标 ⇒ 翻页游标一起丢 ✗。
 *
 * ## 判据（三条）
 *
 * 1. **已有的顺序不动**（Core 时间线顺序不在浏览器复制 —— 与 `mergeOlderPage` 同一条纪律）；
 * 2. **同一 id 以服务端这一份为准**（编辑 / 撤回要跟上；本地乐观态不该压住服务端事实）；
 * 3. **最新一页里没见过的 id 追加在尾部**（它们是更新来的）。
 *
 * ⚠ **通知不吃这一份**：调用方拿"最新一页"去算新增（`trackNewMessages`），
 * 不拿合并后的整条 —— 否则翻出来的历史会被当成新消息**再通知一遍**。
 */
export function mergeLatestPage(existing: Message[], latest: Message[]): Message[] {
  const fresh = new Map(latest.map((m) => [m.id, m]))
  const seen = new Set<string>()
  const result: Message[] = []
  for (const m of existing) {
    if (seen.has(m.id)) continue
    seen.add(m.id)
    result.push(fresh.get(m.id) ?? m)
  }
  for (const m of latest) {
    if (seen.has(m.id)) continue
    seen.add(m.id)
    result.push(m)
  }
  return result
}

/**
 * ★ **一页时间线落进某一间房**：先问「这间房的窗口读过没有」，再决定怎么落
 * （2026-10-20 真机事故：小可 11:29 那条跑到 01:34 那一段**前面**）。
 *
 * ## 病（不是合并函数坏了，是它被喂了不合法的输入）
 *
 * 页面启动**不预加载**任何房间的时间线（缓存是"点开那一间"才读的）。于是：
 * ① 门铃先送来一条新消息，直播间那条路写 `messagesByRoom[room] = [那一条]`
 * —— 那是**片段**，不是时间线；② 用户开房，服务端整页 1..6 才到；
 * ③ `mergeLatestPage` 的口径是"已有顺序不动、没见过的追尾"（它没错，那是
 * 为"翻过更早的历史"写的）⇒ 那**一条**被当成时间线的**开头**，最新那条顶在最上面 ✗。
 *
 * ## 判据（一句话：**片段不是窗口**）
 *
 * - `loaded === true`（这间房读到过一整页，可能还翻过更早）⇒ 照旧 `mergeLatestPage`；
 * - `loaded === false`（从没读到过）⇒ **这一页就是全部**：照它的顺序**替换**。
 *   顺序仍归 Core —— 这里**一个字都不排序**，也不从别处推断顺序；
 * - 不管哪一支：`existing` 里比这一页还新、页里**还没有**的那些（刚发出去的乐观那条、
 *   比这一页迟到的那条）必须**留住**，排在页尾。
 *
 * ⚠ 这条判据替换不掉 `mergeLatestPage`，也不能被它替换：后者服务"翻过历史"，
 * 前者服务"只有片段"。两件事混在一起就会重现这次事故（顺序那一半在 `mergeOlderPage` 里同理）。
 */
export function applyRoomPage(input: {
  existing: Message[]
  page: Message[]
  loaded: boolean
}): Message[] {
  const { existing, page, loaded } = input
  if (loaded) return mergeLatestPage(existing, page)
  // 服务端这一页还没包含的那些（乐观那条 / 比这一页迟到的）：它们是**更新**的，
  // 排页尾。⚠ 不按 `createdAt` 排 —— 顺序不在浏览器复制。
  const known = new Set(page.map((m) => m.id))
  const newerThanPage = existing.filter((m) => !known.has(m.id))
  return [...page, ...newerThanPage]
}

/**
 * ★ **乐观那条收到服务端 id 时改名**——**先去掉服务端已经到手的那一份**（2026-10-16）。
 *
 * ## 病（一个 id 两条 ⇒ 两行画在同一个位置上）
 *
 * 发送时先追加一条**乐观**消息（本地 id），拿到服务端 id 后 `settleSent` 把它**改名**。
 * 但服务端的回显可能**先**到了（WS 唤醒 → 合并那一页把它并了进来）⇒ 改名之后数组里
 * 同一个 id **有两条** ✗。而这一页的列表是**虚拟列表**，行身份 = 消息 id
 * ⇒ 两条同 id ⇒ 同一个 `translateY` ⇒ **两行叠在一起**（真机上看到的就是这个：
 * 顶部那两段字压在一行里）。
 *
 * ⚠ 从前"每次唤醒把整条时间线换成最新一页"恰好把这个重复**盖掉**了；2026-10-16 改成**合并**
 * 之后它就会**留在数组里** ⇒ 这条去重必须同时在场（两件事是一刀，不许只做一件）。
 *
 * 判据：结果里 `serverId` **恰好一条**（乐观那条，带服务端 id、`delivered`），其余原样。
 */
export function settleOptimisticId(
  existing: Message[],
  localId: string,
  serverId: string,
): Message[] {
  return existing
    .filter((m) => m.id !== serverId)
    .map((m) =>
      m.id === localId ? { ...m, id: serverId, delivery: 'delivered' as DeliveryState } : m,
    )
}
