/**
 * **主题这一族的持久面**：明暗模式 / 官方预设 / 配色变体 / 字号（四格，四个键）。
 *
 * ## 为什么单独一层（2026-10-08 存储收尾第二刀）
 *
 * 这四格原先住在 `hooks/use-theme.tsx` 里，**在模块求值期同步读 `localStorage`** ——
 * 那是"首帧必须已经有值"与"存储窄口从第一天起就是异步的"撞在一起的地方。
 * 现在分成两半，各自只说自己的事：
 *
 * - **本模块 = 持久面**：键 / 宽容解析 / 首帧快照 / 补水 / 写；
 * - **`use-theme.tsx` = 呈现面**：DOM 属性、注入 `<style>`、用户主题的动作。
 *
 * ## 判据：先有值再渲染（形状照 `store.ts` 的 `hydrateLocalState`）
 *
 * 1. **模块求值期只有缺省值**（下面的 `snapshot`）—— 窄口是异步的，求值期拿不到文件；
 * 2. `hydrateThemePrefs()`（**幂等**）在 `createRoot` **之前**跑（`main.tsx`）：经窄口读进来，
 *    顺带做**读穿式迁移**（老位置还留着这四格时搬进窄口，判据在 `api/storage.ts`）；
 * 3. Provider 的 `useState` 初始化器读**快照**（同步、一定有值）⇒ **第一帧就是用户自己的配色**；
 * 4. 失败一律降级成缺省值，绝不拦着渲染（白屏比"没读到主题"糟得多）。
 *
 * ⚠ **写是异步的**（`storage.write`）：调用点 fire-and-forget，失败只记一笔日志 ——
 * 那是"这次选的主题不会被记住"这一档损失（与应用/会话状态无关），不值得拦住界面。
 * ⚠ 键名**一字不改**（`celira-theme` 等四格）。改了就等于把用户已经选好的外观丢掉，
 * 而且 `scripts/probe.mjs` / `screenshot-catalog.mjs` 是按这些键钉住主题的（它们走 addInitScript）。
 */

import { readThrough, storage } from '../api/storage.ts'
import {
  isFontSizeId,
  isThemePresetId,
  isThemeVariantId,
  THEME_PRESETS,
  type FontSizeId,
  type ThemeMode,
  type ThemePresetId,
  type ThemeVariantId,
} from './themes.ts'

/** 配色变体按主题各自记忆：`{ [presetId]: variantId }`。 */
export type VariantMap = Partial<Record<ThemePresetId, ThemeVariantId>>

export interface ThemePrefs {
  mode: ThemeMode
  preset: ThemePresetId
  variantMap: VariantMap
  fontSize: FontSizeId
}

const MODE_KEY = 'celira-theme'
const PRESET_KEY = 'celira-theme-preset'
const VARIANT_KEY = 'celira-theme-variant'
const FONT_SIZE_KEY = 'celira-font-size'

/**
 * 缺省（首访 / 读不到时）：深色 + 默认预设 + 原色 + 标准字号。
 *
 * ⚠ 深色是**刻意的默认**：`index.html` 的 `<html class="dark">` 就是它，
 * 于是"没读到偏好"与"默认偏好"落在同一个观感上，不会多闪一次。
 */
export const DEFAULT_THEME_PREFS: ThemePrefs = {
  mode: 'dark',
  preset: 'celira',
  variantMap: {},
  fontSize: 'standard',
}

// ── 宽容解析（四条，各自独立：读不懂哪一格就只退回哪一格）────────────────────

export function parseThemeMode(raw: string | null | undefined): ThemeMode {
  return raw === 'light' || raw === 'system' ? raw : 'dark'
}

export function parseThemePreset(raw: string | null | undefined): ThemePresetId {
  const value = raw ?? null
  return isThemePresetId(value) ? value : 'celira'
}

/** 脏数据一律诚实丢弃：非 JSON、非对象、非法变体 id 都不进入状态。 */
export function parseThemeVariantMap(raw: string | null | undefined): VariantMap {
  if (!raw) return {}
  try {
    const parsed: unknown = JSON.parse(raw)
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) return {}
    const map: VariantMap = {}
    for (const p of THEME_PRESETS) {
      const v = (parsed as Record<string, unknown>)[p.id]
      if (typeof v === 'string' && isThemeVariantId(p.id, v)) map[p.id] = v
    }
    return map
  } catch {
    return {}
  }
}

export function parseFontSize(raw: string | null | undefined): FontSizeId {
  const value = raw ?? null
  return isFontSizeId(value) ? value : 'standard'
}

// ── 首帧快照（模块求值期只有缺省值）＋ 补水 ─────────────────────────────────

let snapshot: ThemePrefs = { ...DEFAULT_THEME_PREFS, variantMap: {} }

/** 这一刻的四格值。**Provider 的初始化器读它**（同步 ⇒ 第一帧就是用户自己的配色）。 */
export function themePrefsSnapshot(): ThemePrefs {
  return snapshot
}

let hydrated: Promise<void> | null = null

/**
 * 补水（**幂等**）：把四格经窄口读进快照。`main.tsx` 在 `createRoot` 之前 await 它。
 *
 * 失败降级成缺省值并留痕 —— 读不到主题比拦住整个应用轻得多。
 */
export function hydrateThemePrefs(): Promise<void> {
  hydrated ??= (async () => {
    try {
      const [modeRaw, presetRaw, variantRaw, fontRaw] = await Promise.all([
        readThrough(MODE_KEY),
        readThrough(PRESET_KEY),
        readThrough(VARIANT_KEY),
        readThrough(FONT_SIZE_KEY),
      ])
      snapshot = {
        mode: parseThemeMode(modeRaw),
        preset: parseThemePreset(presetRaw),
        variantMap: parseThemeVariantMap(variantRaw),
        fontSize: parseFontSize(fontRaw),
      }
    } catch (error) {
      snapshot = { ...DEFAULT_THEME_PREFS, variantMap: {} }
      console.error('[theme] 主题偏好没补上水（按缺省值启动，下次启动再试）', error)
    }
  })()
  return hydrated
}

// ── 写（异步；先改快照，再落盘）─────────────────────────────────────────────

/** 写失败只记一笔：这一档损失是"下次开应用回到默认外观"，不值得打断用户。 */
function warnWrite(key: string, ok: boolean) {
  if (!ok) console.warn(`[theme] ${key} 没能落盘（这次仍然生效，下次启动会回到上一次记住的那份）`)
}

export async function saveThemeMode(mode: ThemeMode): Promise<void> {
  snapshot = { ...snapshot, mode }
  warnWrite(MODE_KEY, await storage.write(MODE_KEY, mode))
}

export async function saveThemePreset(preset: ThemePresetId): Promise<void> {
  snapshot = { ...snapshot, preset }
  warnWrite(PRESET_KEY, await storage.write(PRESET_KEY, preset))
}

export async function saveThemeVariantMap(variantMap: VariantMap): Promise<void> {
  snapshot = { ...snapshot, variantMap }
  warnWrite(VARIANT_KEY, await storage.write(VARIANT_KEY, JSON.stringify(variantMap)))
}

export async function saveThemeFontSize(fontSize: FontSizeId): Promise<void> {
  snapshot = { ...snapshot, fontSize }
  warnWrite(FONT_SIZE_KEY, await storage.write(FONT_SIZE_KEY, fontSize))
}
