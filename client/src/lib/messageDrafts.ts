/**
 * **会话草稿**：客户端私有事实（不进任何公共契约）—— 某个会话里还没发出去的那段字。
 *
 * ## 存储（2026-10-08 存储收尾第二刀）
 *
 * 原先住在 `components/celira/MessageInput.tsx` 里，**模块求值期同步读 `localStorage`**、
 * 每敲一下同步写一次。那是"文件存储接不进来"的一处（壳的 webview 不是家）。现在：
 *
 * 1. **键名一字不改**（`celira_drafts`，一个 `{ [roomId]: body }` 的 JSON 对象）——
 *    改了就等于把用户已经打了一半的话丢掉；
 * 2. 读写都走 `api/storage.ts` 的**窄口**（壳里落 `<app_config_dir>/local-store.json`）；
 * 3. **先有值再渲染**（形状照 `api/store.ts` 的 `hydrateLocalState`）：模块求值期只有空表，
 *    `hydrateDrafts()`（**幂等**）由 `main.tsx` 在 `createRoot` **之前** await，
 *    `MessageInput` 的初始化器读 `draftFor(roomId)`（同步 ⇒ 第一帧就带着上次没发完的那段字）。
 *
 * ⚠ 写是**异步**的（每敲一下一次 fire-and-forget）：写不进去只影响"下次开应用还记不记得"，
 * 本次会话内由这里的 `drafts` 表（内存）保证 —— 为它崩掉一个输入框是不值当的。
 *
 * ⚠ **已知边界（未在本刀处理）**：这一格只按房间 id 分，没有世界作用域（房间 id 是世界内的
 * 标识）。房间 id 是服务端铸的 `uuid v4` ⇒ 两个世界撞同一个 id 的概率可忽略；真要收，得先定
 * "草稿该按世界分还是按机器分"（键名一改，用户手上那份草稿就没了），另作一刀。
 * 同一形状的问题在 `wallpaper.ts` 的按会话覆盖上也成立。
 */

import { readThrough, storage } from '../api/storage.ts'

const DRAFTS_KEY = 'celira_drafts'

/** 宽容解析：读不懂、非对象、空串值一律丢掉（不抛、不返回半个对象）。 */
export function parseDrafts(raw: string | null | undefined): Map<string, string> {
  if (!raw) return new Map()
  try {
    const parsed: unknown = JSON.parse(raw)
    if (parsed === null || typeof parsed !== 'object' || Array.isArray(parsed)) return new Map()
    return new Map(
      Object.entries(parsed as Record<string, unknown>).filter(
        (entry): entry is [string, string] => typeof entry[1] === 'string' && entry[1].length > 0,
      ),
    )
  } catch {
    return new Map()
  }
}

export function serializeDrafts(drafts: ReadonlyMap<string, string>): string {
  return JSON.stringify(Object.fromEntries(drafts))
}

// ── 首帧快照（模块求值期只有空表）＋ 补水 ───────────────────────────────────

const drafts = new Map<string, string>()

/** 某个会话的草稿（没有就是空串）。**`MessageInput` 的初始化器读它**。 */
export function draftFor(roomId: string): string {
  return drafts.get(roomId) ?? ''
}

let hydrated: Promise<void> | null = null

/**
 * 补水（**幂等**）：把草稿经窄口读进快照。`main.tsx` 在 `createRoot` 之前 await 它。
 *
 * 失败降级成空表并留痕 —— 读不到草稿比拦住整个应用轻得多（大不了重打一遍）。
 */
export function hydrateDrafts(): Promise<void> {
  hydrated ??= (async () => {
    try {
      for (const [roomId, body] of parseDrafts(await readThrough(DRAFTS_KEY))) drafts.set(roomId, body)
    } catch (error) {
      console.error('[drafts] 草稿没补上水（这次当作没有草稿，下次启动再试）', error)
    }
  })()
  return hydrated
}

// ── 写（异步；内存先改，落盘排成一条链）──────────────────────────────────────

let warnedWriteFailure = false
/**
 * 落盘排成**一条链**：同一个键的两次写按调用顺序落地。
 *
 * 为什么必须有：窄口是异步的（壳里一次写就是一次 IPC ＋ 一次文件读改写），
 * 而打字是**每敲一下一次**——两条写并发时"后发的先落地"会让盘上留着**上一版**草稿
 * （看着像"少打了几个字"，且下次启动才发现）。链条把顺序钉住，代价只是几次微任务。
 */
let writeChain: Promise<void> = Promise.resolve()

function enqueueWrite(): void {
  writeChain = writeChain.then(async () => {
    try {
      if (await storage.write(DRAFTS_KEY, serializeDrafts(drafts))) return
      if (!warnedWriteFailure) {
        warnedWriteFailure = true
        console.warn('[drafts] 草稿没能落盘（本次会话里仍然在，关掉应用就没了）')
      }
    } catch (error) {
      // ⚠ 窄口的实现按纪律**不抛**（失败如实返回 `false`）；万一有哪一份实现抛了，
      //   这一笔没落盘就算了 —— **绝不能让链条被一次异常毒死**：那样后面每一笔都会静默不写，
      //   而草稿是"用户打了一半的话"，静默丢掉正是这一格最不该有的失败方式。
      console.warn('[drafts] 草稿落盘时抛了（这一笔没落，后面的照旧）', error)
    }
  })
}

/**
 * 记一段草稿（空串 = 删掉这一格）。
 *
 * ⚠ 先 `await hydrateDrafts()` 再改内存、再排队落盘：**不假设调用方补过水** ——
 *   否则一次过早的写入会把盘上别的会话的草稿整份覆盖掉（这张表是"读改写"的）。
 *   补水早已完成时，这一段就是一个微任务（切房前一定已经生效）。
 */
export function setDraft(roomId: string, body: string): void {
  void hydrateDrafts().then(() => {
    if (body) drafts.set(roomId, body)
    else drafts.delete(roomId)
    enqueueWrite()
  })
}
