/**
 * 聊天壁纸：偏好（**经 `api/storage.ts` 的窄口**）+ 自定义图片 Blob（IndexedDB）。
 * 纯外观事实，只存本机，不进 Core/Host；壁纸只作用于聊天页消息区。
 *
 * 存储结构：全局默认 + 按会话覆盖（rooms[roomId] 优先于 global）。
 * dim（蒙版档位）只有全局一档，不按会话拆。
 *
 * ## 先有值再渲染（2026-10-08 存储收尾第二刀）
 *
 * 这两格原先在**模块求值期同步读 `localStorage`**（`useState(loadWallpaperPref)` 那种
 * 初始化器）—— 那是"首帧要有值"与"窄口从第一天起就是异步的"撞在一起的地方。现在：
 * 模块求值期只有缺省值（`snapshot`），`hydrateWallpaper()`（幂等）在 `createRoot` **之前**
 * 由 `main.tsx` await，Provider 的初始化器读快照 ⇒ 第一帧就是用户自己的壁纸。
 *
 * ⚠ 键名**一字不改**（`celira-wallpaper` / `celira-last-room`）：改了就等于把用户已经选好的
 * 壁纸丢掉。写是异步的（fire-and-forget，失败只记一笔日志）。
 */

import { readThrough, storage } from '../api/storage.ts'

export type WallpaperKind = 'none' | 'official' | 'custom'
export type WallpaperDim = 'low' | 'medium' | 'high'

export interface WallpaperSelection {
  kind: WallpaperKind
  /** kind = official 时的官方壁纸 id */
  assetId?: string
}

export interface WallpaperPref {
  global: WallpaperSelection & { dim: WallpaperDim }
  /** 按会话覆盖；存在即优先于 global（含显式 'none' = 此会话不要壁纸） */
  rooms: Record<string, WallpaperSelection>
}

/** 设置页「仅此会话」作用范围要知道用户最后在哪个会话。 */
export interface LastRoom {
  id: string
  name: string
}

export const OFFICIAL_WALLPAPERS = [
  { id: 'aurora', name: '极光', url: '/wallpapers/aurora.svg' },
  { id: 'dunes', name: '沙丘', url: '/wallpapers/dunes.svg' },
  { id: 'night-grid', name: '夜色网格', url: '/wallpapers/night-grid.svg' },
  { id: 'rice-paper', name: '宣纸', url: '/wallpapers/rice-paper.svg' },
] as const

export type OfficialWallpaperId = (typeof OFFICIAL_WALLPAPERS)[number]['id']

const PREF_KEY = 'celira-wallpaper'
const LAST_ROOM_KEY = 'celira-last-room'

const DEFAULT_PREF: WallpaperPref = {
  global: { kind: 'none', dim: 'medium', assetId: undefined },
  rooms: {},
}

function cleanKind(value: unknown): WallpaperKind {
  return value === 'official' || value === 'custom' ? value : 'none'
}

function cleanDim(value: unknown): WallpaperDim {
  return value === 'low' || value === 'high' ? value : 'medium'
}

function cleanSelection(value: unknown): WallpaperSelection {
  const raw = (value ?? {}) as Partial<WallpaperSelection>
  return {
    kind: cleanKind(raw.kind),
    assetId: typeof raw.assetId === 'string' ? raw.assetId : undefined,
  }
}

/** 缺省偏好（每一次都返回新对象：它是可写的 state，不许共享同一份）。 */
function defaultPref(): WallpaperPref {
  return { global: { ...DEFAULT_PREF.global }, rooms: {} }
}

/**
 * 解析持久偏好。**宽容**：读不懂、字段缺失、类型不符一律回退默认值，绝不抛。
 * 旧版扁平结构 `{ kind, dim, assetId }` 迁移为 `{ global, rooms }`（原地读懂，不改盘上那份）。
 */
export function parseWallpaperPref(raw: string | null | undefined): WallpaperPref {
  if (!raw) return defaultPref()
  try {
    const parsed = JSON.parse(raw) as Record<string, unknown>
    if (!parsed.global || typeof parsed.global !== 'object') {
      return { global: { ...cleanSelection(parsed), dim: cleanDim(parsed.dim) }, rooms: {} }
    }
    const roomsRaw = (parsed.rooms ?? {}) as Record<string, unknown>
    const rooms: Record<string, WallpaperSelection> = {}
    for (const [roomId, sel] of Object.entries(roomsRaw)) {
      if (typeof roomId === 'string' && roomId) rooms[roomId] = cleanSelection(sel)
    }
    return {
      global: { ...cleanSelection(parsed.global), dim: cleanDim((parsed.global as { dim?: unknown }).dim) },
      rooms,
    }
  } catch {
    return defaultPref()
  }
}

// ── 首帧快照（模块求值期只有缺省值）＋ 补水 ─────────────────────────────────

let snapshot: WallpaperPref = defaultPref()
let lastRoom: LastRoom | null = null

/** 这一刻的壁纸偏好。**Provider 的初始化器读它**（同步 ⇒ 第一帧就是用户自己的壁纸）。 */
export function wallpaperPrefSnapshot(): WallpaperPref {
  return snapshot
}

/** 设置页「仅此会话」作用范围要知道用户最后在哪个会话（同上：首帧可用）。 */
export function lastRoomSnapshot(): LastRoom | null {
  return lastRoom
}

let hydrated: Promise<void> | null = null

/**
 * 补水（**幂等**）：偏好与"上次所在会话"经窄口读进快照。`main.tsx` 在 `createRoot` 之前 await。
 *
 * ⚠ 自定义壁纸那张图（IndexedDB 里的 Blob）**不在这里**：它是二进制、体积可能大，
 *   由 Provider 挂载后异步取（`loadCustomWallpaper`），读不到就诚实回退为无壁纸。
 */
export function hydrateWallpaper(): Promise<void> {
  hydrated ??= (async () => {
    try {
      const [prefRaw, roomRaw] = await Promise.all([readThrough(PREF_KEY), readThrough(LAST_ROOM_KEY)])
      snapshot = parseWallpaperPref(prefRaw)
      lastRoom = parseLastRoom(roomRaw)
    } catch (error) {
      snapshot = defaultPref()
      lastRoom = null
      console.error('[wallpaper] 壁纸偏好没补上水（按无壁纸启动，下次启动再试）', error)
    }
  })()
  return hydrated
}

/** 写失败只记一笔：这一档损失是"这次选的壁纸下次不在了"，不值得打断用户。 */
function warnWrite(key: string, ok: boolean) {
  if (!ok) console.warn(`[wallpaper] ${key} 没能落盘（这次仍然生效，下次启动会回到上一次记住的那份）`)
}

export async function saveWallpaperPref(pref: WallpaperPref): Promise<boolean> {
  snapshot = pref
  const ok = await storage.write(PREF_KEY, JSON.stringify(pref))
  warnWrite(PREF_KEY, ok)
  return ok
}

/** 解析"上次所在会话"：形状不对 ⇒ `null`（设置页就当没有这一格）。 */
export function parseLastRoom(raw: string | null | undefined): LastRoom | null {
  if (!raw) return null
  try {
    const parsed = JSON.parse(raw) as Partial<LastRoom>
    return typeof parsed.id === 'string' && typeof parsed.name === 'string'
      ? { id: parsed.id, name: parsed.name }
      : null
  } catch {
    return null
  }
}

export async function saveLastRoom(room: LastRoom): Promise<boolean> {
  lastRoom = room
  const ok = await storage.write(LAST_ROOM_KEY, JSON.stringify(room))
  warnWrite(LAST_ROOM_KEY, ok)
  return ok
}

// ── 自定义壁纸本地校验（纯逻辑，可独立单测）──
// 只允许精确的栅格 MIME 类型；官方壁纸走固定 OFFICIAL_WALLPAPERS 目录（SVG），
// 与自定义上传无关。错误分类稳定，不携带文件名或图片字节。
export const WALLPAPER_MAX_BYTES = 10 * 1024 * 1024 // 10 MiB

export const WALLPAPER_ALLOWED_MIME_TYPES = [
  'image/png',
  'image/jpeg',
  'image/webp',
  'image/gif',
] as const

export const WALLPAPER_ACCEPT = WALLPAPER_ALLOWED_MIME_TYPES.join(',')

export type WallpaperValidationError = 'unsupported_type' | 'file_too_large'

export type WallpaperValidationResult = { ok: true } | { ok: false; error: WallpaperValidationError }

export function validateWallpaperBlob(blob: { type: string; size: number }): WallpaperValidationResult {
  if (!(WALLPAPER_ALLOWED_MIME_TYPES as readonly string[]).includes(blob.type)) {
    return { ok: false, error: 'unsupported_type' }
  }
  if (blob.size > WALLPAPER_MAX_BYTES) {
    return { ok: false, error: 'file_too_large' }
  }
  return { ok: true }
}

export class WallpaperRejectedError extends Error {
  readonly code: WallpaperValidationError

  constructor(code: WallpaperValidationError) {
    super(`wallpaper rejected: ${code}`)
    this.name = 'WallpaperRejectedError'
    this.code = code
  }
}

// 自定义壁纸图片 Blob 存 IndexedDB（体积可能大，不进 localStorage）。
// 与 src/api/idb.ts（core 模式消息缓存）无关，独立小库。
const DB_NAME = 'celira-appearance'
const STORE = 'wallpaper'
const BLOB_KEY = 'custom'

function openDb(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const req = indexedDB.open(DB_NAME, 1)
    req.onupgradeneeded = () => req.result.createObjectStore(STORE)
    req.onsuccess = () => resolve(req.result)
    req.onerror = () => reject(req.error)
  })
}

export async function saveCustomWallpaper(blob: Blob): Promise<void> {
  // 持久化前守卫：未来任何调用方都不能绕过 UI 落盘非法图片。
  const check = validateWallpaperBlob(blob)
  if (!check.ok) throw new WallpaperRejectedError(check.error)
  const db = await openDb()
  try {
    await new Promise<void>((resolve, reject) => {
      const tx = db.transaction(STORE, 'readwrite')
      tx.objectStore(STORE).put(blob, BLOB_KEY)
      tx.oncomplete = () => resolve()
      tx.onerror = () => reject(tx.error)
    })
  } finally {
    db.close()
  }
}

export async function loadCustomWallpaper(): Promise<Blob | null> {
  const db = await openDb()
  try {
    return await new Promise<Blob | null>((resolve, reject) => {
      const req = db.transaction(STORE).objectStore(STORE).get(BLOB_KEY)
      req.onsuccess = () => {
        // fail closed：存储内容不通过校验就当作无壁纸，绝不变成 object URL。
        const blob = (req.result as Blob | undefined) ?? null
        resolve(blob && validateWallpaperBlob(blob).ok ? blob : null)
      }
      req.onerror = () => reject(req.error)
    })
  } finally {
    db.close()
  }
}

export async function clearCustomWallpaper(): Promise<void> {
  const db = await openDb()
  try {
    await new Promise<void>((resolve, reject) => {
      const tx = db.transaction(STORE, 'readwrite')
      tx.objectStore(STORE).delete(BLOB_KEY)
      tx.oncomplete = () => resolve()
      tx.onerror = () => reject(tx.error)
    })
  } finally {
    db.close()
  }
}
