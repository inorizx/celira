import type { Message, Room } from '@/api/domain'

interface RoomCache {
  roomId: string
  /**
   * **归档键**：这一份缓存属于**哪个世界**（`home_id`；世界身份还不知道时退回 Human token）。
   *
   * 为什么不直接用 token（2026-09-25 改）：token 是**凭据**，不是**世界** —— 备份/恢复出来的
   * 第二个世界可能带着同一份库、同一支 token，按 token 守卫会让两个世界互读房间与消息。
   * 旧行没有这一格 ⇒ 守卫不通过 ⇒ 当作没有缓存（缓存是尽力而为的，不迁移）。
   */
  worldKey: string
  messages: Message[]
  nextBefore: string | null
  savedAt: number
}

interface RoomsCache {
  roomId: 'rooms'
  worldKey: string
  rooms: Room[]
  savedAt: number
}

const DB_NAME = 'celira-cache'
/** ⚠ 加一个 store 就要升版本（家口远程访问刀：v1 → v2 加了 `homes`）；旧库走 `onupgradeneeded`。 */
const DB_VERSION = 2
const STORE = 'rooms'
/**
 * **家条目**那个 store（家口远程访问刀）。
 *
 * 为什么与本机缓存同一只库：它们都是"这个浏览器自己的东西"（缓存 + 配置），
 * 一间库、两个 store，比再开一只库少一层生命周期。`keyPath: 'id'`（`'local'` / `'home-N'`）。
 */
const HOMES_STORE = 'homes'

let dbPromise: Promise<IDBDatabase> | null = null

function openDb(): Promise<IDBDatabase> {
  if (dbPromise) return dbPromise
  dbPromise = new Promise((resolve, reject) => {
    if (typeof indexedDB === 'undefined') {
      reject(new Error('indexedDB unavailable'))
      return
    }
    const req = indexedDB.open(DB_NAME, DB_VERSION)
    req.onupgradeneeded = () => {
      if (!req.result.objectStoreNames.contains(STORE)) {
        req.result.createObjectStore(STORE, { keyPath: 'roomId' })
      }
      // v2：新 store 只在这一支里建（老库升上来时 `rooms` 已经在了，不会被重建）。
      if (!req.result.objectStoreNames.contains(HOMES_STORE)) {
        req.result.createObjectStore(HOMES_STORE, { keyPath: 'id' })
      }
    }
    req.onsuccess = () => resolve(req.result)
    req.onerror = () => reject(req.error)
  })
  return dbPromise
}

function tx<T>(
  mode: IDBTransactionMode,
  fn: (store: IDBObjectStore) => IDBRequest<T>,
  store: string = STORE,
): Promise<T> {
  return openDb().then(
    (db) =>
      new Promise<T>((resolve, reject) => {
        const t = db.transaction(store, mode)
        const req = fn(t.objectStore(store))
        req.onsuccess = () => resolve(req.result)
        req.onerror = () => reject(req.error)
      }),
  )
}

export async function loadRoomCache(roomId: string, worldKey: string): Promise<RoomCache | null> {
  try {
    const cached = await tx<RoomCache | undefined>('readonly', (store) => store.get(roomId))
    if (!cached || cached.worldKey !== worldKey) return null
    return cached
  } catch {
    return null
  }
}

export async function saveRoomCache(
  roomId: string,
  worldKey: string,
  messages: Message[],
  nextBefore: string | null,
): Promise<void> {
  try {
    await tx('readwrite', (store) =>
      store.put({ roomId, worldKey, messages, nextBefore, savedAt: Date.now() } satisfies RoomCache),
    )
  } catch {
    // cache is best-effort
  }
}

export async function clearRoomCache(roomId: string): Promise<void> {
  try {
    await tx('readwrite', (store) => store.delete(roomId))
  } catch {
    // best-effort
  }
}

export async function saveRoomsCache(worldKey: string, rooms: Room[]): Promise<void> {
  try {
    await tx('readwrite', (store) =>
      store.put({ roomId: 'rooms', worldKey, rooms, savedAt: Date.now() } satisfies RoomsCache),
    )
  } catch {
    // cache is best-effort
  }
}

export async function loadRoomsCache(worldKey: string): Promise<Room[] | null> {
  try {
    const cached = await tx<RoomsCache | undefined>('readonly', (store) => store.get('rooms'))
    if (!cached || cached.worldKey !== worldKey) return null
    return cached.rooms
  } catch {
    return null
  }
}

// ---------------------------------------------------------------------------
// 家条目（家口远程访问刀）：**持久的是原样的记录**，解释与默认值归 `homes.ts`
// ---------------------------------------------------------------------------

/**
 * 读出存着的家条目**原始记录**（形状守卫在 `homes.ts` 的 `parseStoredEntry` 里做 ——
 * 这一层只搬字节，不认识"家条目该长什么样"）。
 *
 * 读不到（浏览器不支持 / 库坏了）⇒ **抛**：由调用方决定"当作只有本机"还是如实报错
 * （静默返回空会被读成"用户没配过家"）。
 */
export async function listHomeEntryRecords(): Promise<unknown[]> {
  const rows = await tx<unknown[]>('readonly', (store) => store.getAll(), HOMES_STORE)
  return Array.isArray(rows) ? rows : []
}

/** 写一条家条目（同 id 覆盖）。失败照抛（写不进去不许静默）。 */
export async function putHomeEntryRecord(record: unknown): Promise<void> {
  await tx('readwrite', (store) => store.put(record as never), HOMES_STORE)
}

/** 删一条家条目（本机那条由 `homes.ts` 拦住，不从这里删）。 */
export async function deleteHomeEntryRecord(id: string): Promise<void> {
  await tx('readwrite', (store) => store.delete(id), HOMES_STORE)
}
